import client from './client';

export type ScriptType = 'request' | 'response' | 'decode' | 'parser';

export interface ScriptInfo {
  name: string;
  script_type: ScriptType;
  description?: string;
  created_at: number;
  updated_at: number;
}

export interface ScriptDetail extends ScriptInfo {
  content: string;
}

export interface ScriptsListResponse {
  request: ScriptInfo[];
  response: ScriptInfo[];
  decode: ScriptInfo[];
  parser: ScriptInfo[];
}

export interface SaveScriptRequest {
  content: string;
  description?: string;
}

export interface TestScriptRequest {
  type: ScriptType;
  content: string;
  mock_request?: {
    url?: string;
    method?: string;
    headers?: Record<string, string>;
    body?: string;
  };
  mock_response?: {
    status?: number;
    headers?: Record<string, string>;
    body?: string;
  };
}

export interface ScriptLogEntry {
  timestamp: number;
  level: 'debug' | 'info' | 'warn' | 'error';
  message: string;
  args?: unknown[];
}

export interface TestRequestModifications {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
}

export interface TestResponseModifications {
  status?: number;
  status_text?: string;
  headers?: Record<string, string>;
  body?: string;
}

export interface DecodeOutput {
  data: string;
  code: string;
  msg: string;
}

export interface ScriptExecutionResult {
  script_name: string;
  script_type: ScriptType;
  success: boolean;
  error?: string;
  duration_ms: number;
  logs: ScriptLogEntry[];
  request_modifications?: TestRequestModifications;
  response_modifications?: TestResponseModifications;
  decode_output?: DecodeOutput;
}

export const scriptsApi = {
  list: async (): Promise<ScriptsListResponse> => {
    const response = await client.get('/scripts');
    return response.data;
  },

  get: async (type: ScriptType, name: string): Promise<ScriptDetail> => {
    const response = await client.get(`/scripts/${type}/${name}`);
    return response.data;
  },

  save: async (type: ScriptType, name: string, data: SaveScriptRequest): Promise<ScriptDetail> => {
    const response = await client.put(`/scripts/${type}/${name}`, data);
    return response.data;
  },

  delete: async (type: ScriptType, name: string): Promise<void> => {
    await client.delete(`/scripts/${type}/${name}`);
  },

  rename: async (type: ScriptType, name: string, newName: string): Promise<void> => {
    await client.post(`/scripts/rename/${type}/${name}`, { new_name: newName });
  },

  test: async (data: TestScriptRequest): Promise<ScriptExecutionResult> => {
    const response = await client.post('/scripts/test', data);
    return response.data;
  },
};

export default scriptsApi;
