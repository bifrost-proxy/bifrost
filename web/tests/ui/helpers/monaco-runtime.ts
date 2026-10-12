import type { Page } from "@playwright/test";
import type { Monaco } from "@monaco-editor/react";

type RuntimeExport = Partial<Monaco> &
  Partial<Monaco["editor"]> &
  Partial<Monaco["languages"]> &
  Partial<Monaco["Uri"]> &
  Partial<Monaco["MarkerSeverity"]>;
type MonacoRuntime = Pick<
  Monaco,
  "editor" | "languages" | "Uri" | "MarkerSeverity"
>;

// Reuse the module the page actually loaded. Bundled Monaco does not install
// window.monaco; importing its existing URL preserves the same editors/workers.
// Locate public APIs by shape instead of depending on minified export names.
export async function getMonacoRuntime(page: Page) {
  return page.evaluateHandle(async () => {
    const modules = Array.from(
      document.querySelectorAll<HTMLScriptElement>(
        'script[type="module"][src]',
      ),
    ).map((script) => script.src);
    const optimizedMonaco = performance
      .getEntriesByType("resource")
      .map((entry) => entry.name)
      .filter((url) => new URL(url).pathname.endsWith("/monaco-editor.js"));
    for (const url of [...optimizedMonaco, ...modules].filter(
      (url) => new URL(url).origin === location.origin,
    )) {
      const module = await import(url);
      const values = Object.values(module) as RuntimeExport[];
      const namespace = values.find(
        (value) =>
          value?.editor?.getEditors &&
          value?.languages?.typescript &&
          value?.Uri?.isUri &&
          value?.MarkerSeverity?.Error === 8,
      );
      if (namespace) return namespace as MonacoRuntime;
      const editor = values.find(
        (value) => typeof value?.getEditors === "function",
      );
      const languages = values.find(
        (value) => value?.typescript?.getTypeScriptWorker,
      );
      const Uri = values.find((value) => value?.parse && value?.isUri);
      const MarkerSeverity = values.find(
        (value) => value?.Error === 8 && value?.Warning === 4,
      );
      if (editor && languages && Uri && MarkerSeverity) {
        return { editor, languages, Uri, MarkerSeverity } as MonacoRuntime;
      }
    }
    throw new Error(
      "The loaded Monaco module does not expose its public editor APIs",
    );
  });
}
