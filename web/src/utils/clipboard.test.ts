import { beforeEach, describe, expect, it, vi } from 'vitest';
import { copyToClipboard } from './clipboard';

vi.mock('../runtime', () => ({
  isDesktopShell: () => false,
}));

function stubExecCommand(implementation: (command: string) => boolean) {
  Object.defineProperty(document, 'execCommand', {
    configurable: true,
    value: vi.fn(implementation),
  });
}

describe('copyToClipboard', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: undefined,
    });
  });

  it('writes through the synchronous copy event fallback', async () => {
    let copiedText = '';
    stubExecCommand((command) => {
      if (command !== 'copy') return false;
      const event = new Event('copy', { cancelable: true }) as ClipboardEvent;
      Object.defineProperty(event, 'clipboardData', {
        configurable: true,
        value: {
          setData: (_type: string, value: string) => {
            copiedText = value;
          },
        },
      });
      document.dispatchEvent(event);
      return true;
    });

    await expect(copyToClipboard('bifrost upgrade')).resolves.toBe(true);
    expect(copiedText).toBe('bifrost upgrade');
  });

  it('does not report success when execCommand returns true without a copy event write', async () => {
    stubExecCommand(() => true);

    await expect(copyToClipboard('bifrost upgrade')).resolves.toBe(false);
  });

  it('falls back to async clipboard when synchronous copy is unavailable', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    stubExecCommand(() => false);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });

    await expect(copyToClipboard('bifrost upgrade')).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith('bifrost upgrade');
  });
});
