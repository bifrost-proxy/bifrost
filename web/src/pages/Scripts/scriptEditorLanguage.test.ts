// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { Monaco } from "@monaco-editor/react";
import type { typescript } from "monaco-editor";
// @ts-expect-error Monaco ships its actual worker as JavaScript without declarations.
import { TypeScriptWorker } from "monaco-editor/esm/vs/language/typescript/tsWorker.js";
import type { ScriptType } from "../../api/scripts";
import {
  configureScriptEditor,
  SCRIPT_RUNTIME_LIB_URI,
} from "./scriptEditorLanguage";

type ExtraLib = { filePath: string; content: string };
type MirrorModel = {
  uri: { path: string; toString: () => string };
  version: number;
  getValue: () => string;
};

function createLanguageHarness() {
  let compilerOptions: typescript.CompilerOptions = {};
  let extraLibs: ExtraLib[] = [];
  let libraryModel: { dispose: () => void } | null = null;
  const defaults = {
    setCompilerOptions: vi.fn((options: typescript.CompilerOptions) => {
      compilerOptions = options;
    }),
    setDiagnosticsOptions: vi.fn(),
    setExtraLibs: vi.fn((libs: ExtraLib[]) => {
      extraLibs = libs;
    }),
  };
  const monaco = {
    languages: {
      typescript: {
        typescriptDefaults: defaults,
        ScriptTarget: { ES2020: 7 },
        ModuleResolutionKind: { NodeJs: 2 },
        ModuleKind: { CommonJS: 1 },
      },
    },
    editor: { getModel: vi.fn(() => libraryModel) },
    Uri: { parse: (uri: string) => uri },
  } as unknown as Monaco;

  const configure = (type: ScriptType) => configureScriptEditor(monaco, type);
  const model = (uri: string, content: string): MirrorModel => ({
    uri: { path: new URL(uri).pathname, toString: () => uri },
    version: 1,
    getValue: () => content,
  });
  const createWorker = (content: string, materializeLibrary = false) => {
    const uri = "file:///script.ts";
    const models = [model(uri, content)];
    if (materializeLibrary)
      models.push(model(extraLibs[0].filePath, extraLibs[0].content));
    const worker: typescript.TypeScriptWorker = new TypeScriptWorker(
      { getMirrorModels: () => models },
      {
        compilerOptions,
        extraLibs: Object.fromEntries(
          extraLibs.map((lib) => [
            lib.filePath,
            { content: lib.content, version: 1 },
          ]),
        ),
        inlayHintsOptions: {},
      },
    );
    return {
      worker,
      diagnostics: async (file = uri) => [
        ...(await worker.getSyntacticDiagnostics(file)),
        ...(await worker.getSemanticDiagnostics(file)),
      ],
    };
  };
  return {
    configure,
    defaults,
    monaco,
    createWorker,
    setLibraryModel: (value: { dispose: () => void }) => {
      libraryModel = value;
    },
  };
}

const validSamples: Record<ScriptType, string> = {
  request:
    'request.method = "POST"; request.headers["x-test"] = ctx.requestId; console.info(request.url);',
  response:
    'response.status = 201; response.headers["x-test"] = response.request.method; console.info(response.body);',
  decode:
    'ctx.output = { data: request.bodyBase64 || response.bodyBase64, code: "0", msg: response.request.path }; console.info(ctx.phase);',
  parser: `function currentRequest() {
    const emptyRequest = { method: "", host: "", path: "", url: "" };
    if (ctx.phase === "request" || ctx.phase === "websocket_send") return request || emptyRequest;
    return response && response.request ? response.request : emptyRequest;
  }
  const req = currentRequest();
  ctx.output = { data: JSON.stringify([req.method, req.host, req.path, req.url]), code: "0", msg: request.bodyBase64 || response.bodyBase64 };
  console.info(ctx.phase === "websocket_recv");`,
};

describe("Scripts Monaco runtime declarations", () => {
  it.each<ScriptType>(["request", "response", "decode", "parser"])(
    "checks %s scripts and a materialized declaration model without conflicting globals",
    async (type) => {
      const harness = createLanguageHarness();
      harness.configure(type);
      const { diagnostics } = harness.createWorker(validSamples[type], true);
      expect(await diagnostics()).toEqual([]);
      expect(await diagnostics(SCRIPT_RUNTIME_LIB_URI)).toEqual([]);
    },
  );

  it("atomically replaces one canonical library and disposes materialized snapshots across type switches", async () => {
    const harness = createLanguageHarness();
    const dispose = vi.fn();
    harness.setLibraryModel({ dispose });
    for (const type of [
      "request",
      "response",
      "decode",
      "parser",
      "request",
      "parser",
    ] as const) {
      harness.configure(type);
      expect(harness.defaults.setExtraLibs).toHaveBeenLastCalledWith([
        { filePath: SCRIPT_RUNTIME_LIB_URI, content: expect.any(String) },
      ]);
      const { diagnostics } = harness.createWorker(validSamples[type], true);
      expect(await diagnostics()).toEqual([]);
      expect(await diagnostics(SCRIPT_RUNTIME_LIB_URI)).toEqual([]);
    }
    expect(dispose).toHaveBeenCalledTimes(6);
    expect(harness.defaults.setExtraLibs).toHaveBeenCalledTimes(6);
    expect(harness.monaco.editor.getModel).toHaveBeenCalledWith(
      SCRIPT_RUNTIME_LIB_URI,
    );
    expect(harness.defaults.setDiagnosticsOptions).toHaveBeenLastCalledWith({
      noSemanticValidation: false,
      noSyntaxValidation: false,
    });
  });

  it("continues reporting invalid fields, readonly assignments, and syntax errors", async () => {
    const harness = createLanguageHarness();
    harness.configure("parser");
    const { worker } = harness.createWorker(
      'request.missingField; request.path = "/changed"; const broken = ;',
    );
    expect(
      (await worker.getSemanticDiagnostics("file:///script.ts")).map(
        (diagnostic) => diagnostic.code,
      ),
    ).toEqual(expect.arrayContaining([2339, 2540]));
    expect(
      await worker.getSyntacticDiagnostics("file:///script.ts"),
    ).not.toEqual([]);
  });

  it.each<ScriptType>(["request", "response", "decode", "parser"])(
    "preserves the real QuickJS timing and microtask globals in %s scripts",
    async (type) => {
      const harness = createLanguageHarness();
      harness.configure(type);
      const { diagnostics } = harness.createWorker(
        "performance = performance; console.info(performance.now(), performance.timeOrigin); queueMicrotask(() => console.info('queued'));",
        true,
      );
      expect(await diagnostics()).toEqual([]);
      expect(await diagnostics(SCRIPT_RUNTIME_LIB_URI)).toEqual([]);
    },
  );

  it("does not advertise browser globals absent from QuickJS", async () => {
    const harness = createLanguageHarness();
    harness.configure("request");
    // Verified against the actual request/response/decode/parser sandboxes.
    const unavailableGlobals = [
      "URL",
      "URLSearchParams",
      "TextEncoder",
      "TextDecoder",
      "atob",
      "btoa",
      "fetch",
      "setTimeout",
      "clearTimeout",
      "setInterval",
      "clearInterval",
      "setImmediate",
      "clearImmediate",
      "requestAnimationFrame",
      "cancelAnimationFrame",
      "document",
      "window",
    ];
    const { diagnostics } = harness.createWorker(
      `console.info(new Map(), Promise.resolve(1)); ${unavailableGlobals.join(";")};`,
    );
    const errors = await diagnostics();
    expect(errors).toHaveLength(unavailableGlobals.length);
    expect(errors.map((diagnostic) => diagnostic.messageText)).toEqual(
      unavailableGlobals.map((name) => expect.stringContaining(name)),
    );
  });

  it("does not hide legitimate property errors caused by an untyped empty fallback", async () => {
    const harness = createLanguageHarness();
    harness.configure("parser");
    const { diagnostics } = harness.createWorker(
      "function currentRequest() { return response && response.request ? response.request : {}; } currentRequest().method;",
    );
    expect((await diagnostics()).map((diagnostic) => diagnostic.code)).toEqual([
      2339,
    ]);
  });
});
