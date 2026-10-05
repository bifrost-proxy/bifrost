import type { Monaco } from "@monaco-editor/react";
import type { ScriptType } from "../../api/scripts";

// A relative extra-lib path becomes a different URI when Monaco opens a type
// definition, making the same globals appear twice in the worker's program.
export const SCRIPT_RUNTIME_LIB_URI = "file:///bifrost-script-runtime.d.ts";

export function configureScriptEditor(monaco: Monaco, scriptType: ScriptType) {
  const { typescriptDefaults, ScriptTarget, ModuleResolutionKind, ModuleKind } =
    monaco.languages.typescript;

  typescriptDefaults.setCompilerOptions({
    target: ScriptTarget.ES2020,
    // Scripts run in QuickJS, without the browser's DOM globals (e.g. console).
    lib: ["lib.es2020.d.ts"],
    allowNonTsExtensions: true,
    moduleResolution: ModuleResolutionKind.NodeJs,
    module: ModuleKind.CommonJS,
    noEmit: true,
    strict: false,
    allowJs: true,
    checkJs: true,
    noImplicitAny: false,
    noUnusedLocals: false,
    noUnusedParameters: false,
  });
  typescriptDefaults.setDiagnosticsOptions({
    noSemanticValidation: false,
    noSyntaxValidation: false,
  });

  // Go to Definition and diagnostic related information can materialize this
  // library as a model. Remove that snapshot before switching script types so
  // the worker cannot prefer stale declarations over the replacement extra lib.
  monaco.editor.getModel(monaco.Uri.parse(SCRIPT_RUNTIME_LIB_URI))?.dispose();
  typescriptDefaults.setExtraLibs([
    {
      filePath: SCRIPT_RUNTIME_LIB_URI,
      content:
        QUICKJS_INTRINSICS +
        (scriptType === "request"
          ? BIFROST_TYPES_REQUEST
          : scriptType === "response"
            ? BIFROST_TYPES_RESPONSE
            : BIFROST_TYPES_DECODE),
    },
  ]);
}

// Context::full in rquickjs 0.9 uses QuickJS-ng's built-in performance object
// and microtask scheduler. Keep those real runtime globals without importing DOM.
const QUICKJS_INTRINSICS = `
declare var performance: {
  readonly timeOrigin: number;
  now(): number;
};
declare function queueMicrotask(callback: () => void): void;
`;

const BIFROST_TYPES_DECODE = `
/**
 * Bifrost Decode Script Types
 *
 * Decode scripts are executed BEFORE body is stored and pushed.
 * - ctx.phase === "request"  : request body decode
 * - ctx.phase === "response" : response body decode (response.request carries request snapshot)
 * - ctx.phase === "websocket_send" : client-to-server WebSocket frame decode
 * - ctx.phase === "websocket_recv" : server-to-client WebSocket frame decode
 */

type BifrostScriptPhase = "request" | "response" | "websocket_send" | "websocket_recv";

interface BifrostDecodeRequest {
  readonly url: string;
  readonly host: string;
  readonly path: string;
  readonly protocol: string;
  readonly clientIp: string;
  readonly clientApp: string | null;
  readonly method: string;
  readonly headers: Record<string, string>;

  /** UTF-8 preview (may be truncated) */
  readonly body: string;
  /** Hex preview (may be truncated) */
  readonly bodyHex: string;
  /** Full body bytes encoded as base64 */
  readonly bodyBase64: string;
  /** Original byte length */
  readonly bodySize: number;
  readonly bodyHexTruncated: boolean;
  readonly bodyTextTruncated: boolean;
}

interface BifrostDecodeResponse {
  readonly status: number;
  readonly statusText: string;
  readonly headers: Record<string, string>;
  readonly body: string;
  readonly bodyHex: string;
  readonly bodyBase64: string;
  readonly bodySize: number;
  readonly bodyHexTruncated: boolean;
  readonly bodyTextTruncated: boolean;
  readonly request: {
    url: string;
    method: string;
    host: string;
    path: string;
    protocol: string;
    clientIp: string;
    clientApp: string | null;
    headers: Record<string, string>;
  };
}

interface BifrostDecodeOutput {
  data: string;
  code: string;
  msg: string;
}

interface BifrostContext {
  readonly requestId: string;
  readonly scriptName: string;
  readonly scriptType: "request" | "response" | "decode" | "parser";
  readonly phase?: BifrostScriptPhase;
  output?: BifrostDecodeOutput;
  readonly values: Record<string, string>;
  readonly matchedRules: Array<{ pattern: string; protocol: string; value: string }>;
}

interface BifrostLog {
  log(...args: any[]): void;
  debug(...args: any[]): void;
  info(...args: any[]): void;
  warn(...args: any[]): void;
  error(...args: any[]): void;
}

interface BifrostFile {
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

interface BifrostNet {
  readonly enabled: boolean;
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

declare const request: BifrostDecodeRequest;
/**
 * Response snapshot for response/websocket_recv phases.
 * Runtime value is null in request/websocket_send phases, so guard by ctx.phase
 * before reading response fields.
 */
declare const response: BifrostDecodeResponse;
declare const ctx: BifrostContext;
declare let output: BifrostDecodeOutput | undefined;
declare const log: BifrostLog;
declare const console: BifrostLog;
declare const file: BifrostFile;
declare const net: BifrostNet;
`;

const BIFROST_TYPES_REQUEST = `
/**
 * Bifrost Request Script Types
 *
 * Request scripts are executed BEFORE the request is sent to the upstream server.
 * You can modify: method, headers, body
 * Read-only properties: url, host, path, protocol, clientIp, clientApp
 */

/** HTTP Request object - available in request scripts */
interface BifrostRequest {
  /** Full request URL (read-only) */
  readonly url: string;
  /** Host name from the request (read-only) */
  readonly host: string;
  /** Request path (read-only) */
  readonly path: string;
  /** Protocol: "http" or "https" (read-only) */
  readonly protocol: string;
  /** Client IP address (read-only) */
  readonly clientIp: string;
  /** Client application identifier, if available (read-only) */
  readonly clientApp: string | null;
  /** HTTP method (GET, POST, PUT, DELETE, etc.) - modifiable */
  method: string;
  /** Request headers as key-value pairs - modifiable */
  headers: Record<string, string>;
  /** Request body content - modifiable */
  body: string | null;
}

/** Script execution context - provides metadata and configuration */
interface BifrostContext {
  /** Unique identifier for this request */
  readonly requestId: string;
  /** Name of the current script */
  readonly scriptName: string;
  /** Type of script: "request" | "response" | "decode" | "parser" */
  readonly scriptType: "request" | "response" | "decode" | "parser";
  /** Current phase for decode/parser scripts */
  readonly phase?: "request" | "response" | "websocket_send" | "websocket_recv";
  /** Custom key-value configuration from Bifrost settings */
  readonly values: Record<string, string>;
  /** List of rules that matched this request */
  readonly matchedRules: Array<{
    /** Rule pattern (e.g., "*.example.com") */
    pattern: string;
    /** Protocol (http/https) */
    protocol: string;
    /** Rule value/target */
    value: string;
  }>;
}

/** Logging interface - logs are captured and displayed in test results */
interface BifrostLog {
  /** Log a message (alias for info) */
  log(...args: any[]): void;
  /** Log debug level message */
  debug(...args: any[]): void;
  /** Log info level message */
  info(...args: any[]): void;
  /** Log warning level message */
  warn(...args: any[]): void;
  /** Log error level message */
  error(...args: any[]): void;
}

/** Sandbox file API (path is relative to scripts/_sandbox) */
interface BifrostFile {
  /** Whether file APIs are enabled */
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

/** Network request API (returns JSON string, use JSON.parse) */
interface BifrostNet {
  /** Whether net APIs are enabled */
  readonly enabled: boolean;
  /**
   * net.fetch(url, optionsJson?) -> JSON string
   * optionsJson example: {"method":"POST","headers":{"Content-Type":"application/json"},"body":"...","timeoutMs":3000}
   */
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

/** The request object to inspect and modify */
declare const request: BifrostRequest;
/** Script execution context with metadata */
declare const ctx: BifrostContext;
/** Logging interface */
declare const log: BifrostLog;
/** Console logging (alias for log) */
declare const console: BifrostLog;
/** File API */
declare const file: BifrostFile;
/** Network API */
declare const net: BifrostNet;
`;

const BIFROST_TYPES_RESPONSE = `
/**
 * Bifrost Response Script Types
 *
 * Response scripts are executed AFTER receiving the response from upstream.
 * You can modify: status, statusText, headers, body
 * Read-only: request (original request information)
 */

/** HTTP Response object - available in response scripts */
interface BifrostResponse {
  /** HTTP status code (e.g., 200, 404, 500) - modifiable */
  status: number;
  /** HTTP status text (e.g., "OK", "Not Found") - modifiable */
  statusText: string;
  /** Response headers as key-value pairs - modifiable */
  headers: Record<string, string>;
  /** Response body content - modifiable */
  body: string | null;
  /** Original request information (read-only) */
  readonly request: {
    /** Full request URL */
    url: string;
    /** HTTP method used */
    method: string;
    /** Host name */
    host: string;
    /** Request path */
    path: string;
    /** Request protocol, when available */
    protocol?: string;
    /** Client IP address, when available */
    clientIp?: string;
    /** Client application identifier, if available */
    clientApp?: string | null;
    /** Request headers */
    headers: Record<string, string>;
  };
}

/** Script execution context - provides metadata and configuration */
interface BifrostContext {
  /** Unique identifier for this request */
  readonly requestId: string;
  /** Name of the current script */
  readonly scriptName: string;
  /** Type of script: "request" | "response" | "decode" | "parser" */
  readonly scriptType: "request" | "response" | "decode" | "parser";
  /** Current phase for decode/parser scripts */
  readonly phase?: "request" | "response" | "websocket_send" | "websocket_recv";
  /** Custom key-value configuration from Bifrost settings */
  readonly values: Record<string, string>;
  /** List of rules that matched this request */
  readonly matchedRules: Array<{
    /** Rule pattern (e.g., "*.example.com") */
    pattern: string;
    /** Protocol (http/https) */
    protocol: string;
    /** Rule value/target */
    value: string;
  }>;
}

/** Logging interface - logs are captured and displayed in test results */
interface BifrostLog {
  /** Log a message (alias for info) */
  log(...args: any[]): void;
  /** Log debug level message */
  debug(...args: any[]): void;
  /** Log info level message */
  info(...args: any[]): void;
  /** Log warning level message */
  warn(...args: any[]): void;
  /** Log error level message */
  error(...args: any[]): void;
}

/** Sandbox file API (path is relative to scripts/_sandbox) */
interface BifrostFile {
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

/** Network request API (returns JSON string, use JSON.parse) */
interface BifrostNet {
  readonly enabled: boolean;
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

/** The response object to inspect and modify */
declare const response: BifrostResponse;
/** Script execution context with metadata */
declare const ctx: BifrostContext;
/** Logging interface */
declare const log: BifrostLog;
/** Console logging (alias for log) */
declare const console: BifrostLog;
/** File API */
declare const file: BifrostFile;
/** Network API */
declare const net: BifrostNet;
`;
