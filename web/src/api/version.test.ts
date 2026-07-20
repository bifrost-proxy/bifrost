import { beforeEach, describe, expect, it, vi } from 'vitest';

const apiMocks = vi.hoisted(() => ({
  post: vi.fn(),
}));
const desktopMocks = vi.hoisted(() => ({
  issueDesktopUpgradeOriginToken: vi.fn(),
}));

vi.mock('./client', () => ({
  get: vi.fn(),
  post: apiMocks.post,
}));
vi.mock('../desktop/tauri', () => desktopMocks);

import {
  DESKTOP_UPGRADE_ORIGIN_HEADER,
  startUpgrade,
} from './version';

describe('upgrade request origin', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.post.mockResolvedValue({ phase: 'checking' });
  });

  it('does not issue a desktop credential for CLI upgrades', async () => {
    await startUpgrade('cli');

    expect(desktopMocks.issueDesktopUpgradeOriginToken).not.toHaveBeenCalled();
    expect(apiMocks.post).toHaveBeenCalledWith('/system/upgrade');
  });

  it('attaches a one-time Tauri-issued credential to desktop upgrades', async () => {
    desktopMocks.issueDesktopUpgradeOriginToken.mockResolvedValue('desktop-token');

    await startUpgrade('desktop');

    expect(desktopMocks.issueDesktopUpgradeOriginToken).toHaveBeenCalledTimes(1);
    expect(apiMocks.post).toHaveBeenCalledWith(
      '/system/upgrade?channel=desktop',
      undefined,
      {
        headers: {
          [DESKTOP_UPGRADE_ORIGIN_HEADER]: 'desktop-token',
        },
      },
    );
  });
});
