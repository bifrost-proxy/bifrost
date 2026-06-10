import { editor, MarkerSeverity } from 'monaco-editor';
import { setFixesForMarker, clearFixesCache, type CodeFix } from './codeAction';
import { apiFetch } from '../../../api/apiFetch';

const MARKER_OWNER = 'bifrost-validation';

export interface ParseError {
  line: number;
  start_column: number;
  end_column: number;
  message: string;
  severity: 'error' | 'warning' | 'info' | 'hint';
  suggestion?: string;
  code?: string;
  related_info?: VariableInfo;
  fixes?: CodeFix[];
}

export interface VariableInfo {
  name: string;
  source: string;
  defined_at?: number;
}

export interface ScriptReference {
  name: string;
  script_type: string;
  line: number;
}

export interface ValidationResult {
  valid: boolean;
  rule_count: number;
  errors: ParseError[];
  warnings: ParseError[];
  defined_variables: VariableInfo[];
  script_references: ScriptReference[];
}

function severityToMarkerSeverity(severity: ParseError['severity']): MarkerSeverity {
  switch (severity) {
    case 'error':
      return MarkerSeverity.Error;
    case 'warning':
      return MarkerSeverity.Warning;
    case 'info':
      return MarkerSeverity.Info;
    case 'hint':
      return MarkerSeverity.Hint;
    default:
      return MarkerSeverity.Error;
  }
}

export async function validateRules(
  content: string,
  globalValues?: Record<string, string>,
  currentRuleName?: string | null,
  currentGroupName?: string | null
): Promise<ValidationResult> {
  const response = await apiFetch('/_bifrost/api/rules/validate', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      content,
      global_values: globalValues || {},
      current_rule_name: currentRuleName || undefined,
      current_group_name: currentGroupName || undefined,
    }),
  });

  if (!response.ok) {
    throw new Error(`Validation request failed: ${response.status}`);
  }

  return response.json();
}

export function setValidationMarkers(
  model: editor.ITextModel,
  result: ValidationResult
): void {
  if (model.isDisposed()) return;

  const modelUri = model.uri.toString();
  clearFixesCache(modelUri);

  const allIssues = [...result.errors, ...result.warnings];
  const markers = allIssues.map((error) => {
    if (error.fixes && error.fixes.length > 0) {
      setFixesForMarker(
        modelUri,
        error.line,
        error.start_column,
        error.end_column + 1,
        error.fixes
      );
    }

    return {
      severity: severityToMarkerSeverity(error.severity),
      message: error.suggestion
        ? `${error.message}\n\nSuggestion: ${error.suggestion}`
        : error.message,
      startLineNumber: error.line,
      startColumn: error.start_column,
      endLineNumber: error.line,
      endColumn: error.end_column + 1,
      code: error.code || undefined,
      source: 'bifrost',
    };
  });

  editor.setModelMarkers(model, MARKER_OWNER, markers);
}

export function clearValidationMarkers(model: editor.ITextModel): void {
  if (model.isDisposed()) return;
  editor.setModelMarkers(model, MARKER_OWNER, []);
}

export type GlobalValuesGetter = () => Record<string, string>;

export interface DebouncedValidator {
  validate: (model: editor.ITextModel, onComplete?: (result: ValidationResult) => void) => void;
  cancel: () => void;
}

export function createDebouncedValidator(
  delayMs = 500,
  getGlobalValues?: GlobalValuesGetter,
  getCurrentRuleName?: () => string | null | undefined,
  getCurrentGroupName?: () => string | null | undefined
): DebouncedValidator {
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  let abortController: AbortController | null = null;

  const cancel = () => {
    if (timeoutId) {
      clearTimeout(timeoutId);
      timeoutId = null;
    }
    if (abortController) {
      abortController.abort();
      abortController = null;
    }
  };

  const validate = (
    model: editor.ITextModel,
    onComplete?: (result: ValidationResult) => void
  ) => {
    cancel();

    if (model.isDisposed()) return;

    timeoutId = setTimeout(async () => {
      if (model.isDisposed()) return;

      abortController = new AbortController();
      const content = model.getValue();
      const globalValues = getGlobalValues?.() || {};

      try {
        const result = await validateRules(
          content,
          globalValues,
          getCurrentRuleName?.(),
          getCurrentGroupName?.()
        );
        if (model.isDisposed()) return;

        setValidationMarkers(model, result);
        onComplete?.(result);
      } catch (error) {
        if (error instanceof Error && error.name === 'AbortError') {
          return;
        }
        console.error('Validation error:', error);
        clearValidationMarkers(model);
      }
    }, delayMs);
  };

  return { validate, cancel };
}
