export interface ProtocolDoc {
  name: string;
  description: string;
  valueType: string;
  valueDescription: string;
  valueSyntax?: string;
  examples: string[];
  category: 'redirect' | 'response' | 'request' | 'performance' | 'header' | 'script' | 'filter' | 'log' | 'other';
}

export const BREAKPOINT_VALUE_COMPLETIONS = [
  {
    value: 'request',
    detail: 'Pause matched requests before sending upstream',
    documentation: 'Requires the global Breakpoint switch to be on. Only traffic matching this rule pauses in the request phase.',
  },
  {
    value: 'req',
    detail: 'Alias for request',
    documentation: 'Requires the global Breakpoint switch to be on. This is a short alias for the request phase.',
  },
  {
    value: 'response',
    detail: 'Pause matched responses before returning to the client',
    documentation: 'Requires the global Breakpoint switch to be on. Only traffic matching this rule pauses in the response phase.',
  },
  {
    value: 'res',
    detail: 'Alias for response',
    documentation: 'Requires the global Breakpoint switch to be on. This is a short alias for the response phase.',
  },
  {
    value: 'request,response',
    detail: 'Pause both request and response phases',
    documentation: 'Use this for a two-step breakpoint. The request must be resumed before the response phase can pause.',
  },
  {
    value: 'both',
    detail: 'Alias for request,response',
    documentation: 'Enables both request and response phase breakpoints for matched traffic.',
  },
  {
    value: 'all',
    detail: 'Alias for request,response',
    documentation: 'Enables all supported breakpoint phases for matched traffic.',
  },
];

export const PROTOCOL_DOCS: Record<string, ProtocolDoc> = {
  statusCode: {
    name: 'statusCode',
    description: 'Return the HTTP status code directly without contacting upstream',
    valueType: 'number',
    valueDescription: 'HTTP status code (100-599)',
    examples: ['statusCode://200', 'statusCode://404', 'statusCode://500'],
    category: 'response',
  },
  replaceStatus: {
    name: 'replaceStatus',
    description: 'Replace the HTTP response status code from upstream',
    valueType: 'number',
    valueDescription: 'HTTP status code (100-599)',
    examples: ['replaceStatus://200', 'replaceStatus://302'],
    category: 'response',
  },
  host: {
    name: 'host',
    description: 'Forward the request to a different host',
    valueType: 'host[:port]',
    valueDescription: 'Target hostname or IP with optional port',
    examples: ['host://127.0.0.1:8080', 'host://api.example.com', 'host://localhost:3000'],
    category: 'redirect',
  },
  xHost: {
    name: 'xHost',
    description: 'Forward request to different host while preserving original Host header',
    valueType: 'host[:port]',
    valueDescription: 'Target hostname or IP with optional port',
    examples: ['xHost://127.0.0.1:8080', 'xHost://backend.local:3000'],
    category: 'redirect',
  },
  method: {
    name: 'method',
    description: 'Override the HTTP request method',
    valueType: 'HTTP method',
    valueDescription: 'Standard HTTP method name',
    examples: ['method://GET', 'method://POST', 'method://PUT', 'method://DELETE'],
    category: 'request',
  },
  file: {
    name: 'file',
    description: 'Serve response from a local file',
    valueType: 'file path',
    valueDescription: 'Absolute or relative path to local file',
    examples: ['file:///path/to/response.json', 'file://./mock/data.json'],
    category: 'response',
  },
  resBody: {
    name: 'resBody',
    description: 'Set or replace the response body',
    valueType: 'text',
    valueDescription: 'Response body content',
    valueSyntax: 'Use `value` for content with spaces, (value) for inline content without spaces',
    examples: ['resBody://simple', 'resBody://`{"code": 0}`', 'resBody://({varName})'],
    category: 'response',
  },
  reqBody: {
    name: 'reqBody',
    description: 'Set or replace the request body',
    valueType: 'text',
    valueDescription: 'Request body content',
    valueSyntax: 'Use `value` for content with spaces, (value) for inline content without spaces',
    examples: ['reqBody://simple', 'reqBody://`{"key": "value"}`', 'reqBody://({varName})'],
    category: 'request',
  },
  resHeaders: {
    name: 'resHeaders',
    description: 'Add or modify response headers',
    valueType: 'key: value',
    valueDescription: 'Header name and value pairs',
    valueSyntax: 'Use `key: value` for headers with spaces, (key:value) for inline without spaces',
    examples: ['resHeaders://(X-Custom:value)', 'resHeaders://`Content-Type: application/json`'],
    category: 'header',
  },
  reqHeaders: {
    name: 'reqHeaders',
    description: 'Add or modify request headers',
    valueType: 'key: value',
    valueDescription: 'Header name and value pairs',
    valueSyntax: 'Use `key: value` for headers with spaces, (key:value) for inline without spaces',
    examples: ['reqHeaders://(X-Debug:true)', 'reqHeaders://`Authorization: Bearer xxx`'],
    category: 'header',
  },
  resType: {
    name: 'resType',
    description: 'Set the response Content-Type header',
    valueType: 'mime type or shortcut',
    valueDescription: 'MIME type or shortcut (json, text, html, xml)',
    examples: ['resType://json', 'resType://text', 'resType://html', 'resType://application/xml'],
    category: 'header',
  },
  reqType: {
    name: 'reqType',
    description: 'Set the request Content-Type header',
    valueType: 'mime type or shortcut',
    valueDescription: 'MIME type or shortcut (json, form, urlencoded)',
    examples: ['reqType://json', 'reqType://form', 'reqType://multipart/form-data'],
    category: 'header',
  },
  cache: {
    name: 'cache',
    description: 'Control response caching',
    valueType: 'seconds | no | no-cache | no-store',
    valueDescription: 'Cache duration in seconds or cache control directive',
    examples: ['cache://3600', 'cache://no', 'cache://no-cache', 'cache://no-store'],
    category: 'performance',
  },
  reqDelay: {
    name: 'reqDelay',
    description: 'Delay the request before sending to upstream',
    valueType: 'milliseconds',
    valueDescription: 'Delay time in milliseconds',
    examples: ['reqDelay://1000', 'reqDelay://500', 'reqDelay://2000'],
    category: 'performance',
  },
  resDelay: {
    name: 'resDelay',
    description: 'Delay the response before sending to client',
    valueType: 'milliseconds',
    valueDescription: 'Delay time in milliseconds',
    examples: ['resDelay://1000', 'resDelay://500', 'resDelay://3000'],
    category: 'performance',
  },
  reqSpeed: {
    name: 'reqSpeed',
    description: 'Limit the request upload speed',
    valueType: 'bytes/second',
    valueDescription: 'Speed limit in bytes per second',
    examples: ['reqSpeed://1024', 'reqSpeed://10240', 'reqSpeed://102400'],
    category: 'performance',
  },
  resSpeed: {
    name: 'resSpeed',
    description: 'Limit the response download speed',
    valueType: 'bytes/second',
    valueDescription: 'Speed limit in bytes per second',
    examples: ['resSpeed://1024', 'resSpeed://10240', 'resSpeed://102400'],
    category: 'performance',
  },
  dns: {
    name: 'dns',
    description: 'Override DNS resolution for the host',
    valueType: 'IP address',
    valueDescription: 'Target IP address (IPv4 or IPv6)',
    examples: ['dns://127.0.0.1', 'dns://192.168.1.100', 'dns://::1'],
    category: 'redirect',
  },
  reqScript: {
    name: 'reqScript',
    description: 'Execute a request modification script',
    valueType: 'inline block or script name',
    valueDescription: 'Prefer an inline block reference for portable rules; named Scripts are also supported',
    valueSyntax: 'Recommended: reqScript://{request_bridge} with a ```request_bridge code block in the same rule file.',
    examples: ['reqScript://{request_bridge}', 'reqScript://addAuth'],
    category: 'script',
  },
  resScript: {
    name: 'resScript',
    description: 'Execute a response modification script',
    valueType: 'inline block or script name',
    valueDescription: 'Prefer an inline block reference for portable rules; named Scripts are also supported',
    valueSyntax: 'Recommended: resScript://{response_bridge} with a ```response_bridge code block in the same rule file.',
    examples: ['resScript://{response_bridge}', 'resScript://processResponse'],
    category: 'script',
  },
  resStreamScript: {
    name: 'resStreamScript',
    description: 'Generate or transform a response as a true incremental SSE stream',
    valueType: 'inline block or response script name',
    valueDescription: 'A persistent per-response stream script; inline blocks are recommended for sharing',
    valueSyntax: 'Use stream.mode = "mock" with stream.next(), or stream.mode = "transform" with stream.onEvent(event). Each returned output is sent immediately.',
    examples: ['resStreamScript://{sse_stream_bridge}', 'resStreamScript://streamAdapter'],
    category: 'script',
  },
  decode: {
    name: 'decode',
    description: 'Decode captured request and response bodies before they are stored, displayed, and searched',
    valueType: 'decode script',
    valueDescription: 'Built-in decoder name or custom decode script name',
    valueSyntax: 'Use decode://utf8 for UTF-8 text, decode://default as the UTF-8 alias, or decode://bp with a bp:// parser rule.',
    examples: [
      'decode://utf8',
      'decode://default',
      'bp://build_in_bp?psm=example.service&method=GetItem decode://bp',
    ],
    category: 'script',
  },
  bp: {
    name: 'bp',
    description: 'Bind a parser script for binary protocol bodies; pair it with decode://bp so decoded content is stored, shown, and searchable',
    valueType: 'parser script',
    valueDescription: 'Parser script name or remote parser URL, with optional query parameters passed to the parser',
    valueSyntax: 'Use bp://build_in_bp?psm=<psm>&method=<method> decode://bp for the built-in parser flow, or bp://<parser-script> decode://bp for another parser.',
    examples: [
      'bp://build_in_bp?psm=example.service&method=GetItem decode://bp',
      'bp://team/custom_parser decode://bp',
      'bp://https://example.com/parser.js?sha256=<sha256> decode://bp',
    ],
    category: 'script',
  },
  breakpoint: {
    name: 'breakpoint',
    description: 'Authorize matched traffic to pause in the Breakpoint UI when the global Breakpoint switch is enabled',
    valueType: 'request | response | request,response | req | res | both | all',
    valueDescription: 'Breakpoint phase for the matched rule. Empty or unknown values do not pause traffic.',
    valueSyntax: 'Use breakpoint://request for request phase, breakpoint://response for response phase, or breakpoint://request,response for both. req/res/both/all are supported aliases. The toolbar switch is only the global gate.',
    examples: [
      'breakpoint://request',
      'breakpoint://response',
      'breakpoint://request,response',
    ],
    category: 'other',
  },
  includeFilter: {
    name: 'includeFilter',
    description: 'Only match requests that pass the filter',
    valueType: 'filter expression',
    valueDescription: 'Filter by headers, cookies, body content, etc.',
    examples: ['includeFilter://h:Content-Type=json', 'includeFilter://c:session=abc', 'includeFilter://b:keyword'],
    category: 'filter',
  },
  excludeFilter: {
    name: 'excludeFilter',
    description: 'Exclude requests that match the filter',
    valueType: 'filter expression',
    valueDescription: 'Filter by headers, cookies, body content, etc.',
    examples: ['excludeFilter://h:X-Skip=true', 'excludeFilter://m:GET', 'excludeFilter://b:ignore'],
    category: 'filter',
  },
  log: {
    name: 'log',
    description: 'Log request and response details',
    valueType: 'log options',
    valueDescription: 'Logging configuration',
    examples: ['log://req', 'log://res', 'log://all'],
    category: 'log',
  },
  disable: {
    name: 'disable',
    description: 'Disable the rule (comment out)',
    valueType: 'none',
    valueDescription: 'No value needed',
    examples: ['disable://'],
    category: 'other',
  },
  enable: {
    name: 'enable',
    description: 'Enable the rule',
    valueType: 'none',
    valueDescription: 'No value needed',
    examples: ['enable://'],
    category: 'other',
  },
  proxy: {
    name: 'proxy',
    description: 'Route request through another proxy',
    valueType: 'proxy URL',
    valueDescription: 'Upstream proxy URL (http/https/socks5), supports user:password@ authentication',
    valueSyntax: 'proxy://[user:password@]host:port or proxy://socks5://[user:password@]host:port',
    examples: [
      'proxy://127.0.0.1:8888',
      'proxy://user:password@127.0.0.1:8888',
      'proxy://socks5://127.0.0.1:1080',
      'proxy://socks5://user:password@127.0.0.1:1080',
      'proxy://http://user:password@proxy.example.com:3128',
    ],
    category: 'redirect',
  },
  http: {
    name: 'http',
    description: 'Redirect to HTTP URL',
    valueType: 'URL',
    valueDescription: 'Target URL or path',
    examples: ['http://example.com', 'http://localhost:3000/api'],
    category: 'redirect',
  },
  https: {
    name: 'https',
    description: 'Redirect to HTTPS URL',
    valueType: 'URL',
    valueDescription: 'Target URL or path',
    examples: ['https://example.com', 'https://api.example.com/v1'],
    category: 'redirect',
  },
  redirect: {
    name: 'redirect',
    description: 'Send HTTP redirect response (302)',
    valueType: 'URL',
    valueDescription: 'Redirect target URL',
    examples: ['redirect://https://new-site.com', 'redirect://https://example.com/new-path'],
    category: 'redirect',
  },
  resCors: {
    name: 'resCors',
    description: 'Add CORS headers to response',
    valueType: 'origin or *',
    valueDescription: 'Allowed origin or * for all',
    examples: ['resCors://*', 'resCors://https://example.com'],
    category: 'header',
  },
  resCookies: {
    name: 'resCookies',
    description: 'Add cookies to response',
    valueType: 'cookie string',
    valueDescription: 'Cookie name=value pairs',
    examples: ['resCookies://(session=abc123)', 'resCookies://(user=test; path=/)'],
    category: 'header',
  },
  reqCookies: {
    name: 'reqCookies',
    description: 'Add cookies to request',
    valueType: 'cookie string',
    valueDescription: 'Cookie name=value pairs',
    examples: ['reqCookies://(auth=token123)', 'reqCookies://(session=abc)'],
    category: 'header',
  },
  resCharset: {
    name: 'resCharset',
    description: 'Set response character encoding',
    valueType: 'charset',
    valueDescription: 'Character encoding name',
    examples: ['resCharset://utf-8', 'resCharset://gbk', 'resCharset://iso-8859-1'],
    category: 'header',
  },
  reqCharset: {
    name: 'reqCharset',
    description: 'Set request character encoding',
    valueType: 'charset',
    valueDescription: 'Character encoding name',
    examples: ['reqCharset://utf-8', 'reqCharset://gbk', 'reqCharset://iso-8859-1'],
    category: 'header',
  },
  trailers: {
    name: 'trailers',
    description: 'Add HTTP trailer headers to response',
    valueType: 'key: value',
    valueDescription: 'Trailer header name and value pairs',
    valueSyntax: 'Use {key:value} or {key:value,key2:value2} for multiple trailers',
    examples: ['trailers://({X-Checksum:abc123})', 'trailers://({Server-Timing:total;dur=123})'],
    category: 'header',
  },
  urlParams: {
    name: 'urlParams',
    description: 'Add query parameters to URL',
    valueType: 'query string',
    valueDescription: 'URL query parameters',
    examples: ['urlParams://(debug=true)', 'urlParams://(page=1&size=10)'],
    category: 'request',
  },
  pathReplace: {
    name: 'pathReplace',
    description: 'Replace URL path using regex',
    valueType: 'pattern/replacement',
    valueDescription: 'Regex pattern and replacement string',
    examples: ['pathReplace://(/api/v1)/api/v2', 'pathReplace://(/old)/new'],
    category: 'redirect',
  },
  reqPrepend: {
    name: 'reqPrepend',
    description: 'Prepend content to request body',
    valueType: 'text',
    valueDescription: 'Content to prepend',
    examples: ['reqPrepend://({"wrapper":)', 'reqPrepend://PREFIX_'],
    category: 'request',
  },
  reqAppend: {
    name: 'reqAppend',
    description: 'Append content to request body',
    valueType: 'text',
    valueDescription: 'Content to append',
    examples: ['reqAppend://(})', 'reqAppend://_SUFFIX'],
    category: 'request',
  },
  resPrepend: {
    name: 'resPrepend',
    description: 'Prepend content to response body',
    valueType: 'text',
    valueDescription: 'Content to prepend',
    examples: ['resPrepend://(/* header */)', 'resPrepend://<!DOCTYPE html>'],
    category: 'response',
  },
  resAppend: {
    name: 'resAppend',
    description: 'Append content to response body',
    valueType: 'text',
    valueDescription: 'Content to append',
    examples: ['resAppend://(/* footer */)', 'resAppend://<script>console.log("injected")</script>'],
    category: 'response',
  },
  resReplace: {
    name: 'resReplace',
    description: 'Replace content in response body',
    valueType: 'pattern/replacement',
    valueDescription: 'Search pattern and replacement',
    valueSyntax: 'Use `pattern`/`replacement` for content with spaces',
    examples: ['resReplace://old/new', 'resReplace://`old text`/`new text`'],
    category: 'response',
  },
  reqReplace: {
    name: 'reqReplace',
    description: 'Replace content in request body',
    valueType: 'pattern/replacement',
    valueDescription: 'Search pattern and replacement',
    examples: ['reqReplace://test/prod', 'reqReplace://dev/staging'],
    category: 'request',
  },
  resEmpty: {
    name: 'resEmpty',
    description: 'Return empty response body',
    valueType: 'none',
    valueDescription: 'No value needed',
    examples: ['resEmpty://'],
    category: 'response',
  },
  urlReplace: {
    name: 'urlReplace',
    description: 'Replace URL path using regex pattern',
    valueType: 'pattern/replacement',
    valueDescription: 'Regex pattern and replacement string',
    examples: ['urlReplace://(/api/v1)/api/v2', 'urlReplace://(/old)/new'],
    category: 'redirect',
  },
  attachment: {
    name: 'attachment',
    description: 'Force download with Content-Disposition: attachment header',
    valueType: 'filename (optional)',
    valueDescription: 'Optional filename for download',
    examples: ['attachment://', 'attachment://file.zip'],
    category: 'response',
  },
  params: {
    name: 'params',
    description: 'Add or merge URL query parameters',
    valueType: 'query string',
    valueDescription: 'URL query parameters to add',
    examples: ['params://(debug=true)', 'params://(page=1&size=10)'],
    category: 'request',
  },
  htmlAppend: {
    name: 'htmlAppend',
    description: 'Append HTML content to HTML response body',
    valueType: 'HTML content',
    valueDescription: 'HTML content to append',
    valueSyntax: 'Use `content` for content with spaces',
    examples: ['htmlAppend://`<script>alert(1)</script>`'],
    category: 'response',
  },
  jsAppend: {
    name: 'jsAppend',
    description: 'Append JavaScript code to JS response',
    valueType: 'JavaScript code',
    valueDescription: 'JavaScript code to append',
    valueSyntax: 'Use `content` for content with spaces',
    examples: ['jsAppend://`console.log("injected")`'],
    category: 'response',
  },
  cssAppend: {
    name: 'cssAppend',
    description: 'Append CSS styles to CSS response',
    valueType: 'CSS code',
    valueDescription: 'CSS code to append',
    valueSyntax: 'Use `content` for content with spaces',
    examples: ['cssAppend://`body { color: red; }`'],
    category: 'response',
  },
};

export const HTTP_STATUS_CODES = [
  { code: 100, label: '100 Continue' },
  { code: 101, label: '101 Switching Protocols' },
  { code: 200, label: '200 OK' },
  { code: 201, label: '201 Created' },
  { code: 204, label: '204 No Content' },
  { code: 301, label: '301 Moved Permanently' },
  { code: 302, label: '302 Found' },
  { code: 303, label: '303 See Other' },
  { code: 304, label: '304 Not Modified' },
  { code: 307, label: '307 Temporary Redirect' },
  { code: 308, label: '308 Permanent Redirect' },
  { code: 400, label: '400 Bad Request' },
  { code: 401, label: '401 Unauthorized' },
  { code: 403, label: '403 Forbidden' },
  { code: 404, label: '404 Not Found' },
  { code: 405, label: '405 Method Not Allowed' },
  { code: 408, label: '408 Request Timeout' },
  { code: 409, label: '409 Conflict' },
  { code: 410, label: '410 Gone' },
  { code: 422, label: '422 Unprocessable Entity' },
  { code: 429, label: '429 Too Many Requests' },
  { code: 500, label: '500 Internal Server Error' },
  { code: 501, label: '501 Not Implemented' },
  { code: 502, label: '502 Bad Gateway' },
  { code: 503, label: '503 Service Unavailable' },
  { code: 504, label: '504 Gateway Timeout' },
];

export const HTTP_METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', 'TRACE', 'CONNECT'];

export const CACHE_VALUES = [
  { value: '60', label: '1 minute' },
  { value: '300', label: '5 minutes' },
  { value: '600', label: '10 minutes' },
  { value: '1800', label: '30 minutes' },
  { value: '3600', label: '1 hour' },
  { value: '86400', label: '1 day' },
  { value: 'no', label: 'Disable caching' },
  { value: 'no-cache', label: 'Revalidate before use' },
  { value: 'no-store', label: 'Never store' },
];

export const CONTENT_TYPES = [
  { value: 'json', label: 'JSON (application/json)' },
  { value: 'text', label: 'Plain Text (text/plain)' },
  { value: 'html', label: 'HTML (text/html)' },
  { value: 'xml', label: 'XML (application/xml)' },
  { value: 'form', label: 'Form (application/x-www-form-urlencoded)' },
  { value: 'multipart', label: 'Multipart (multipart/form-data)' },
];

const PROTOCOL_ALIASES: Record<string, string> = {
  pathReplace: 'urlReplace',
  download: 'attachment',
  'http-proxy': 'proxy',
  status: 'statusCode',
  hosts: 'host',
  html: 'htmlAppend',
  js: 'jsAppend',
  reqMerge: 'params',
  css: 'cssAppend',
};

function resolveProtocolAlias(name: string): string {
  return PROTOCOL_ALIASES[name] || name;
}

function getAliasesForProtocol(protocolName: string): string[] {
  const aliases: string[] = [];
  for (const [alias, target] of Object.entries(PROTOCOL_ALIASES)) {
    if (target === protocolName || target.toLowerCase() === protocolName.toLowerCase()) {
      aliases.push(alias);
    }
  }
  return aliases;
}

export interface ProtocolDocResult {
  doc: ProtocolDoc;
  isAlias: boolean;
  aliasName?: string;
  aliases: string[];
}

export function getProtocolDoc(protocolName: string): ProtocolDocResult | undefined {
  const resolved = resolveProtocolAlias(protocolName);
  const isAlias = resolved !== protocolName;

  let doc: ProtocolDoc | undefined;
  if (PROTOCOL_DOCS[resolved]) {
    doc = PROTOCOL_DOCS[resolved];
  } else {
    const lowerName = resolved.toLowerCase();
    for (const key of Object.keys(PROTOCOL_DOCS)) {
      if (key.toLowerCase() === lowerName) {
        doc = PROTOCOL_DOCS[key];
        break;
      }
    }
  }

  if (!doc) return undefined;

  return {
    doc,
    isAlias,
    aliasName: isAlias ? protocolName : undefined,
    aliases: getAliasesForProtocol(doc.name),
  };
}

export function formatProtocolHover(result: ProtocolDocResult): string {
  const { doc, isAlias, aliasName, aliases } = result;
  const lines: string[] = [
    `### ${doc.name}://`,
  ];
  if (isAlias && aliasName) {
    lines.push('', `*\`${aliasName}://\` is an alias for \`${doc.name}://\`*`);
  }
  lines.push('', doc.description);
  lines.push('', `**Value:** \`${doc.valueType}\` - ${doc.valueDescription}`);
  if (doc.valueSyntax) {
    lines.push('', `**Syntax:** ${doc.valueSyntax}`);
  }
  if (aliases.length > 0) {
    lines.push('', `**Aliases:** ${aliases.map(a => `\`${a}://\``).join(', ')}`);
  }
  lines.push('', '**Examples:**', '```');
  lines.push(...doc.examples);
  lines.push('```');
  return lines.join('\n');
}
