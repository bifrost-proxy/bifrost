export const bytesFromBase64 = (base64: string): Uint8Array => {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
};

export const toHexViewFromBytes = (buffer: Uint8Array): string => {
  const lines: string[] = [];

  for (let i = 0; i < buffer.length; i += 16) {
    const slice = buffer.slice(i, i + 16);
    const hex = Array.from(slice)
      .map((b) => b.toString(16).padStart(2, '0'))
      .join(' ');
    const ascii = Array.from(slice)
      .map((b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : '.'))
      .join('');
    lines.push(
      `${i.toString(16).padStart(8, '0')}  ${hex.padEnd(48)}  ${ascii}`
    );
  }

  return lines.join('\n');
};

export const toHexView = (text: string): string => {
  const encoder = new TextEncoder();
  const buffer = encoder.encode(text);
  return toHexViewFromBytes(buffer);
};
