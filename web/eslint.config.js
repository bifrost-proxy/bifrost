import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist', 'dist-gzip', 'playwright-report', 'test-results']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      'react-hooks/incompatible-library': 'off',
      'react-hooks/preserve-manual-memoization': 'off',
      'react-hooks/set-state-in-effect': 'off',
      'react-refresh/only-export-components': 'off',
    },
  },
  {
    // Force every browser-origin request through a CSRF-aware transport.
    // `apiFetch` (web/src/api/apiFetch.ts) and the shared axios `client`
    // (web/src/api/client.ts) inject `X-Bifrost-CSRF` on unsafe methods.
    // Importing raw `axios` anywhere else silently bypasses that and makes
    // POST/PUT/PATCH/DELETE requests 403 with "Missing or invalid admin CSRF token".
    files: ['src/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: 'axios',
              message:
                'Do not import axios directly. Use apiFetch (src/api/apiFetch.ts) or the shared client (src/api/client.ts) so X-Bifrost-CSRF is injected on unsafe requests. Only src/api/client.ts may import axios.',
            },
          ],
        },
      ],
    },
  },
  {
    // The shared axios client is the single sanctioned axios entry point;
    // it owns the request interceptor that injects the CSRF token.
    files: ['src/api/client.ts'],
    rules: {
      'no-restricted-imports': 'off',
    },
  },
])
