import axios, { AxiosError } from 'axios';
import type { AxiosRequestConfig } from 'axios';
import { message } from 'antd';
import { getClientId } from '../services/clientId';
import { buildApiUrl, buildAppRouteUrl } from '../runtime';
import { clearAdminToken, getAdminToken } from '../services/adminAuth';
import {
  isDesktopCoreTransitionActive,
  useDesktopCoreStore,
} from '../stores/useDesktopCoreStore';

const client = axios.create({
  timeout: 30000,
  headers: {
    'Content-Type': 'application/json',
  },
});

const GLOBAL_API_MESSAGE_KEY = 'global-api-error';

type ApiErrorPayload = {
  kind: 'connection' | 'business';
  message: string;
  status?: number;
};

function extractErrorMessage(error: AxiosError): string {
  if (
    error.response?.data &&
    typeof error.response.data === 'object' &&
    ('message' in error.response.data || 'error' in error.response.data)
  ) {
    const data = error.response.data as { message?: string; error?: string };
    return String(data.message || data.error || error.message);
  }

  if (typeof error.response?.data === 'string' && error.response.data.trim()) {
    return error.response.data;
  }

  return error.message;
}

function isReadRequest(method?: string): boolean {
  const normalized = method?.toUpperCase();
  return normalized === 'GET' || normalized === 'HEAD';
}

export function getApiErrorPayload(error: unknown): ApiErrorPayload {
  if (!axios.isAxiosError(error)) {
    return {
      kind: 'business',
      message: error instanceof Error ? error.message : String(error),
    };
  }

  const status = error.response?.status;
  const requestMethod = error.config?.method;
  const readyOnce = useDesktopCoreStore.getState().readyOnce;
  const isConnectionIssue =
    !error.response ||
    error.code === 'ERR_NETWORK' ||
    error.code === 'ECONNABORTED' ||
    (!readyOnce && !!status && status >= 500 && isReadRequest(requestMethod));

  return {
    kind: isConnectionIssue ? 'connection' : 'business',
    message: isConnectionIssue
      ? 'Bifrost core is starting. Reconnecting the interface...'
      : extractErrorMessage(error),
    status,
  };
}

export function isConnectionIssueError(error: unknown): boolean {
  return getApiErrorPayload(error).kind === 'connection';
}

export function isNotFoundError(error: unknown): boolean {
  if (axios.isAxiosError(error)) {
    return error.response?.status === 404;
  }
  return false;
}

export function normalizeApiErrorMessage(
  error: unknown,
  fallback = 'Request failed',
): string {
  const payload = getApiErrorPayload(error);
  return payload.message || fallback;
}

export function notifyApiBusinessError(
  error: unknown,
  fallback = 'Request failed',
): void {
  const payload = getApiErrorPayload(error);
  if (payload.kind === 'connection') {
    return;
  }

  message.open({
    key: GLOBAL_API_MESSAGE_KEY,
    type: 'error',
    content: payload.message || fallback,
  });
}

client.interceptors.request.use((config) => {
  config.baseURL = buildApiUrl();
  config.headers = config.headers ?? {};
  config.headers['X-Client-Id'] = getClientId();

  const token = getAdminToken();
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

client.interceptors.response.use(
  (response) => {
    useDesktopCoreStore.getState().markReady();
    useDesktopCoreStore.getState().resolveBooting();
    return response;
  },
  (error: AxiosError) => {
    const payload = getApiErrorPayload(error);

    if (error.response?.status === 401) {
      const url = String(error.config?.url ?? '');
      const isAuthEndpoint = url.includes('/auth/login') || url.includes('/auth/status');
      const isOnLoginPage =
        window.location.pathname.includes('/login') || window.location.hash.includes('/login');
      if (!isAuthEndpoint && !isOnLoginPage) {
        clearAdminToken();
        const next = window.location.hash
          ? window.location.hash.replace(/^#/, '')
          : `${window.location.pathname}${window.location.search}`;
        window.location.assign(
          buildAppRouteUrl(`/login?next=${encodeURIComponent(next || '/traffic')}`),
        );
      }
      return Promise.reject(error);
    }

    if (payload.kind === 'connection') {
      useDesktopCoreStore.getState().showBooting(payload.message);
      return Promise.reject(error);
    }

    if (!isDesktopCoreTransitionActive()) {
      console.error('[API Error]', payload.message);
    }
    return Promise.reject(error);
  }
);

export async function get<T>(url: string, config?: AxiosRequestConfig): Promise<T> {
  const response = await client.get<T>(url, config);
  return response.data;
}

export async function post<T>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T> {
  const response = await client.post<T>(url, data, config);
  return response.data;
}

export async function put<T>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T> {
  const response = await client.put<T>(url, data, config);
  return response.data;
}

export async function del<T>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T> {
  const response = await client.delete<T>(url, { ...config, data });
  return response.data;
}

export async function patch<T>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T> {
  const response = await client.patch<T>(url, data, config);
  return response.data;
}

export default client;
