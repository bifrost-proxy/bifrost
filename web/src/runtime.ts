const DEFAULT_ADMIN_PREFIX = '/_bifrost';
const ADMIN_VIRTUAL_HOST = 'bifrost.local';
const DEFAULT_BACKEND_PORT = 9900;

type DesktopPlatform = 'macos' | 'windows' | 'linux' | 'web';

const desktopRuntime = {
  initialized: false,
  expectedProxyPort: DEFAULT_BACKEND_PORT,
  proxyPort: DEFAULT_BACKEND_PORT,
  platform: 'web' as DesktopPlatform,
};

async function loadDesktopRuntimeSnapshot(): Promise<void> {
  const { getDesktopRuntime } = await import('./desktop/tauri');
  const runtime = await getDesktopRuntime();
  desktopRuntime.expectedProxyPort = runtime.expectedProxyPort;
  desktopRuntime.proxyPort = runtime.proxyPort;
  desktopRuntime.platform = normalizeDesktopPlatform(runtime.platform);

  if (runtime.startupError) {
    console.error('[desktop-runtime] Core reported a startup error during snapshot.', runtime.startupError);
  }
}

export function normalizeDesktopPlatform(platform: string): DesktopPlatform {
  return platform === 'darwin' || platform === 'macos'
    ? 'macos'
    : platform === 'win32' || platform === 'windows'
      ? 'windows'
      : platform === 'linux'
        ? 'linux'
        : 'web';
}

export function isDesktopShell(): boolean {
  return import.meta.env.MODE === 'desktop';
}

export function getDesktopPlatform(): DesktopPlatform {
  return desktopRuntime.platform;
}

export function setDesktopProxyPort(port: number): void {
  desktopRuntime.proxyPort = port;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export function getExpectedDesktopProxyPort(): number {
  return desktopRuntime.expectedProxyPort;
}

export async function initializeDesktopRuntime(): Promise<void> {
  if (!isDesktopShell() || desktopRuntime.initialized) {
    desktopRuntime.initialized = true;
    return;
  }

  try {
    await loadDesktopRuntimeSnapshot();
  } catch (error) {
    console.error(
      '[desktop-runtime] Failed to initialize Tauri runtime, falling back to default port 9900.',
      error,
    );
  } finally {
    desktopRuntime.initialized = true;
  }
}

export function isVirtualHostAccess(): boolean {
  return window.location.hostname.toLowerCase() === ADMIN_VIRTUAL_HOST;
}

export function getAdminPrefix(): string {
  if (isVirtualHostAccess()) {
    return '';
  }
  return DEFAULT_ADMIN_PREFIX;
}

export function getBackendOrigin(): string {
  if (!isDesktopShell()) {
    return window.location.origin;
  }

  return `http://127.0.0.1:${desktopRuntime.proxyPort}`;
}

export function buildBackendUrl(path: string): string {
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  if (isVirtualHostAccess()) {
    return `${getBackendOrigin()}${normalizedPath}`;
  }
  const adminPrefix = getAdminPrefix();
  const resolvedPath = normalizedPath.startsWith(adminPrefix)
    ? normalizedPath
    : `${adminPrefix}${normalizedPath}`;
  return `${getBackendOrigin()}${resolvedPath}`;
}

export function buildApiUrl(path = ''): string {
  const suffix = path.startsWith('/') ? path : `/${path}`;
  return buildBackendUrl(`/api${suffix === '/' ? '' : suffix}`);
}

export function buildPublicUrl(path = ''): string {
  const suffix = path.startsWith('/') ? path : `/${path}`;
  return buildBackendUrl(`/public${suffix === '/' ? '' : suffix}`);
}

export function buildAppRouteUrl(path = ''): string {
  const suffix = path.startsWith('/') ? path : `/${path}`;

  if (isDesktopShell()) {
    const url = new URL(window.location.href);
    url.search = '';
    url.hash = `#${suffix}`;
    return url.toString();
  }

  if (isVirtualHostAccess()) {
    return new URL(suffix, window.location.origin).toString();
  }

  return new URL(`${getAdminPrefix()}${suffix}`, window.location.origin).toString();
}

export function buildWsUrl(path: string, params?: URLSearchParams): string {
  const url = new URL(buildBackendUrl(path));
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  if (params) {
    url.search = params.toString();
  }
  return url.toString();
}

export async function waitForDesktopBackendReady(
  port: number,
  timeoutMs = 8_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const url = `http://127.0.0.1:${port}${DEFAULT_ADMIN_PREFIX}/api/system/overview`;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, {
        method: 'GET',
        cache: 'no-store',
      });
      if (response.ok) {
        return;
      }
    } catch {
      // The listener is still switching over; retry until timeout.
    }

    await delay(150);
  }

  throw new Error(`Timed out waiting for Bifrost core on port ${port}`);
}

export function resolveRequestUrl(input: RequestInfo | URL): RequestInfo | URL {
  if (typeof input !== 'string') {
    return input;
  }

  if (/^[a-zA-Z][a-zA-Z\d+\-.]*:/.test(input)) {
    return input;
  }

  if (input.startsWith('/')) {
    return buildBackendUrl(input);
  }

  return input;
}
