import type { DebugConsoleValue } from "../../../api/devtools";

export function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  return JSON.stringify(value, null, 2);
}

export function consoleValueFromRuntimeResult(value: unknown): DebugConsoleValue {
  if (!value || typeof value !== "object") {
    return consoleValueFromPlain(value);
  }
  const payload = value as { result?: unknown; type?: string; subtype?: string; value?: unknown; description?: string };
  const object = payload.result && typeof payload.result === "object"
    ? payload.result as { type?: string; subtype?: string; value?: unknown; description?: string }
    : payload;
  if ("value" in object) return consoleValueFromPlain(object.value);
  return {
    type: object.type || "object",
    subtype: object.subtype,
    preview: object.description || formatValue(value),
    raw: object.description || formatValue(value),
  };
}

function consoleValueFromPlain(value: unknown): DebugConsoleValue {
  if (value === null) return { type: "null", value, preview: "null", raw: "null" };
  if (value === undefined) return { type: "undefined", preview: "undefined", raw: "undefined" };
  const type = typeof value;
  if (type === "string") return { type, value, preview: JSON.stringify(value), raw: value as string };
  if (type === "number" || type === "boolean") return { type, value, preview: String(value), raw: String(value) };
  try {
    return { type: "object", preview: JSON.stringify(value), raw: JSON.stringify(value, null, 2) };
  } catch {
    return { type: "object", preview: String(value), raw: String(value) };
  }
}
