import operator from './operator';
import {
  dynamicProvider,
  updateDynamicData,
  getDynamicData,
  setLocalVariablesGetter,
  type DynamicCompletionData,
  type ReferenceLocation,
  type ReferenceType,
  type NavigationType,
  type LocalVariableGetter,
} from './dynamic';
import {
  hoverProvider,
  definitionProvider,
  setNavigateCallback,
  getNavigateCallback,
  setLocalVariables,
  setRuleReferenceErrors,
  executePendingNavigation,
  type NavigateCallback,
  type LocalVariableDefinition,
} from './hover';
import {
  validateRules,
  setValidationMarkers,
  clearValidationMarkers,
  createDebouncedValidator,
  type ParseError,
  type ValidationResult,
  type DebouncedValidator,
  type GlobalValuesGetter,
} from './validation';
import {
  codeActionProvider,
  setFixesForMarker,
  clearFixesCache,
  type CodeFix,
} from './codeAction';
import { signatureHelpProvider } from './signature';
import {
  PROTOCOL_DOCS,
  HTTP_STATUS_CODES,
  HTTP_METHODS,
  CACHE_VALUES,
  CONTENT_TYPES,
  getProtocolDoc,
  formatProtocolHover,
  type ProtocolDoc,
} from './protocol-docs';

export { updateDynamicData, getDynamicData, dynamicProvider, setLocalVariablesGetter };
export { hoverProvider, definitionProvider, setNavigateCallback, getNavigateCallback, setLocalVariables, setRuleReferenceErrors, executePendingNavigation };
export { validateRules, setValidationMarkers, clearValidationMarkers, createDebouncedValidator };
export { codeActionProvider, setFixesForMarker, clearFixesCache };
export { signatureHelpProvider };
export { PROTOCOL_DOCS, HTTP_STATUS_CODES, HTTP_METHODS, CACHE_VALUES, CONTENT_TYPES, getProtocolDoc, formatProtocolHover };
export type { DynamicCompletionData, ReferenceLocation, ReferenceType, NavigationType, NavigateCallback, LocalVariableDefinition, LocalVariableGetter };
export type { ParseError, ValidationResult, DebouncedValidator, GlobalValuesGetter, ProtocolDoc, CodeFix };

export default {
  operator,
  dynamicProvider,
  hoverProvider,
  definitionProvider,
  codeActionProvider,
  signatureHelpProvider,
};
