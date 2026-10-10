export function breakpointBodyBytes(
  body: string,
  encoding: "utf8" | "base64",
): Uint8Array {
  if (encoding === "utf8") return new TextEncoder().encode(body);
  if (
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      body,
    )
  )
    throw new Error("Enter valid padded Base64 without whitespace");
  const decoded = atob(body);
  if (btoa(decoded) !== body) throw new Error("Enter canonical Base64");
  return Uint8Array.from(decoded, (value) => value.charCodeAt(0));
}
export function convertBreakpointBody(
  body: string,
  from: "utf8" | "base64",
  to: "utf8" | "base64",
): string {
  if (from === to) return body;
  const bytes = breakpointBodyBytes(body, from);
  if (to === "utf8")
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
