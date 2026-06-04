import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const webPort = Number(process.env.WEB_PORT ?? 3000);
const defaultBackendPort = 9900;

const readArg = (key: string): string | undefined => {
  const prefixed = `${key}=`;
  const kv = process.argv.find((arg) => arg.startsWith(prefixed));
  if (kv) return kv.slice(prefixed.length);
  const idx = process.argv.indexOf(key);
  if (idx !== -1) return process.argv[idx + 1];
  return undefined;
};

const backendPort = (() => {
  const raw =
    readArg('--backend-port') ??
    readArg('--proxy-port') ??
    process.env.BACKEND_PORT ??
    process.env.PROXY_TARGET_PORT ??
    String(defaultBackendPort);

  const n = Number(raw);
  if (!Number.isFinite(n) || n <= 0) return defaultBackendPort;
  return n;
})();

const backendHttpTarget = `http://127.0.0.1:${backendPort}`;
const backendWsTarget = `ws://127.0.0.1:${backendPort}`;

export default defineConfig(({ mode }) => {
  const isDesktop = mode === 'desktop';

  return {
    plugins: [react()],
    optimizeDeps: {
      include: ['monaco-editor'],
    },
    base: isDesktop ? './' : '/_bifrost/',
    build: {
      outDir: isDesktop ? 'dist-desktop' : 'dist',
      emptyOutDir: true,
    },
    define: {
      __BIFROST_DESKTOP__: JSON.stringify(isDesktop),
    },
    server: {
      host: '127.0.0.1',
      port: webPort,
      proxy: {
        '/_bifrost/api': {
          target: backendHttpTarget,
          changeOrigin: true,
          ws: true,
        },
        '/_bifrost/swagger': {
          target: backendHttpTarget,
          changeOrigin: true,
        },
        '/_bifrost/public': {
          target: backendHttpTarget,
          changeOrigin: true,
        },
        '/_bifrost/ws': {
          target: backendWsTarget,
          ws: true,
        },
      },
    },
  };
});
