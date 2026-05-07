import { describe, expect, it } from 'vitest';
import { bytesFromBase64, toHexViewFromBytes } from './hex';

describe('HexView binary helpers', () => {
  it('renders base64 bytes without UTF-8 lossy re-encoding', () => {
    const bytes = bytesFromBase64('gAEAAQAAAAdIZWFsdGh6AAAABwwAAQAA');

    expect(toHexViewFromBytes(bytes)).toContain(
      '00000000  80 01 00 01 00 00 00 07 48 65 61 6c 74 68 7a 00   ........Healthz.',
    );
    expect(toHexViewFromBytes(bytes)).toContain(
      '00000010  00 00 07 0c 00 01 00 00                           ........',
    );
  });
});
