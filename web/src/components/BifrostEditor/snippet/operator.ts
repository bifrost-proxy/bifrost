import { editor, languages, Position } from 'monaco-editor';
import type { IRange } from 'monaco-editor';
import {
  fetchSyntaxInfo,
  getCachedSyntaxInfo,
  type ProtocolInfo,
  type TemplateVariableInfo,
  type UnifiedSyntaxInfo,
  type ProtocolValueSpec,
} from './syntaxApi';
import {
  HTTP_STATUS_CODES,
  HTTP_METHODS,
  CACHE_VALUES,
  CONTENT_TYPES,
  getProtocolDoc,
} from './protocol-docs';

interface Operator extends Partial<languages.CompletionItem> {
  snippet: string[];
}

const inline_tpl_small = '(${1:key}=${2:value})';
const inline_tpl_long = '(${1:key1}=${2:value1}&${3:key2}=${4:value2})';
const block_tpl = `{\${1:block_var}}
\`\`\`\${1:block_var}
\${2:data}
\`\`\`
`;

const genSnippet = (operator: string) => {
  return [
    operator + '://' + inline_tpl_small,
    operator + '://' + inline_tpl_long,
    operator + '://' + block_tpl,
    operator + '://{${1:block_var}}',
  ];
};
const genSnippetBlock = (operator: string) => {
  return [
    operator + '://' + '(${1:value})',
    operator + '://' + block_tpl,
    operator + '://{${1:block_var}}',
  ];
};
const genSnippetReplace = (operator: string) => {
  return [
    operator + '://{${1:block_var}}',
    operator +
    `://{\${1:block_var}}
\`\`\`\${1:block_var}
\${2:str}: \${3:replacement}
/user=([^&])/ig: user=\\$1\\$1
\`\`\`
`,
  ];
};
const genSnippetInlineAndVar = (operator: string) => {
  return [
    operator + '://(${1:inline_value})',
    operator + '://' + block_tpl,
    operator + '://{${1:block_var}}',
  ];
};
const genSnippetFilenameAndVar = (operator: string) => {
  return [
    operator + '://${1:filename}',
    operator + '://' + block_tpl,
    operator + '://{${1:block_var}}',
  ];
};

function getSnippetsForProtocol(
  protocol: ProtocolInfo,
  syntaxInfo: UnifiedSyntaxInfo
): string[] {
  const name = protocol.name;
  const valueType = protocol.value_type;

  if (name.toLowerCase() === 'devtools') {
    return [`${name}://`];
  }

  switch (valueType) {
    case 'headers':
      return genSnippet(name);
    case 'body_content':
      return genSnippetBlock(name);
    case 'old/new/':
      return genSnippetReplace(name);
    case 'file_path':
      return genSnippetFilenameAndVar(name);
    case 'script_name': {
      const scripts =
        name === 'reqScript'
          ? syntaxInfo.scripts.request_scripts
          : syntaxInfo.scripts.response_scripts;

      if (scripts.length > 0) {
        return scripts.map((s) => `${name}://${s.name}`);
      }
      return [`${name}://\${1:script_name}`];
    }
    case 'content':
      return genSnippetFilenameAndVar(name);
    case 'key=value':
      return genSnippet(name);
    case 'host:port':
      return [`${name}://\${1:ip}:\${2:port}`, `${name}://\${1:ip}`];
    case 'proxy_url':
      return [
        `${name}://\${1:ip}:\${2:port}`,
        `${name}://\${1:user}:\${2:password}@\${3:ip}:\${4:port}`,
        `${name}://socks5://\${1:ip}:\${2:port}`,
        `${name}://socks5://\${1:user}:\${2:password}@\${3:ip}:\${4:port}`,
      ];
    case 'url':
      return [`${name}://\${1:ip}:\${2:port}`, `${name}://\${1:hostname}`];
    case 'milliseconds':
      return [`${name}://\${1:ms}`];
    case 'kb_per_second':
      return [`${name}://\${1:kbs}`];
    case 'seconds':
      return [`${name}://\${1:seconds}`];
    case 'status_code':
      return [
        `${name}://200`,
        `${name}://301`,
        `${name}://302`,
        `${name}://400`,
        `${name}://401`,
        `${name}://403`,
        `${name}://404`,
        `${name}://500`,
        `${name}://\${1:code}`,
      ];
    case 'method_name':
      return ['get', 'post', 'put', 'delete', 'patch', 'options', 'head'].map(
        (m) => `${name}://${m}`
      );
    case 'content_type':
      return ['json', 'xml', 'text', 'form', 'urlencoded', 'multipart'].map(
        (t) => `${name}://${t}`
      );
    case 'charset':
      return [`${name}://utf-8`, `${name}://\${1:charset}`];
    case 'empty':
      return [`${name}://`];
    case 'string':
      return genSnippetInlineAndVar(name);
    case 'ip_address':
      return [`${name}://\${1:ip}`];
    case 'dns_server':
      return [`${name}://\${1:ip}`];
    case 'callback_name':
      return [`${name}://\${1:callback}`];
    default:
      return [`${name}://\${1:value}`];
  }
}

function protocolToOperator(
  protocol: ProtocolInfo,
  syntaxInfo: UnifiedSyntaxInfo
): Operator {
  return {
    label: protocol.name,
    detail: `${protocol.name}://<${protocol.value_type}> - ${protocol.description}`,
    snippet: getSnippetsForProtocol(protocol, syntaxInfo),
  };
}

function templateVariableToOperator(variable: TemplateVariableInfo): Operator {
  const name = variable.name;
  let snippets: string[];

  if (name.includes('.xxx') || name.includes('(')) {
    const baseName = name.replace('.xxx', '').replace('(n)', '').replace('(n1-n2)', '');
    if (name.includes('.xxx')) {
      snippets = [`\${${baseName}.\${1:name}}`];
    } else if (name.includes('(n1-n2)')) {
      snippets = [`\${randomInt(\${1:min}-\${2:max})}`];
    } else if (name.includes('(n)')) {
      snippets = [`\${randomInt(\${1:max})}`];
    } else {
      snippets = [`\${${baseName}}`];
    }
  } else {
    snippets = [`\${${name}}`];
  }

  return {
    label: `\${${name}}`,
    detail: `${variable.description} - Example: ${variable.example}`,
    snippet: snippets,
  };
}

function buildFilterOperators(filterSpecs: ProtocolValueSpec[]): Operator[] {
  const operators: Operator[] = [];

  for (const spec of filterSpecs) {
    const allCompletions: string[] = [];
    for (const hint of spec.hints) {
      allCompletions.push(...hint.completions);
    }

    operators.push({
      label: spec.protocol,
      detail: `${spec.protocol}://<${spec.value_format}> - Filter rules`,
      snippet: allCompletions.map((c) => `${spec.protocol}://${c}`),
    });
  }

  operators.push({
    label: 'lineProps',
    detail: 'lineProps://<options>',
    snippet: [
      'lineProps://important',
      'lineProps://disabled',
      'lineProps://important,disabled',
    ],
  });

  return operators;
}

const staticTemplateExtras: Operator[] = [
  {
    label: '${{url-encode}}',
    detail: 'URL encode the value',
    snippet: ['${{${1:hostname}}}'],
  },
  {
    label: '$${escape}',
    detail: 'Escape $ to prevent variable expansion',
    snippet: ['$${${1:varname}}'],
  },
  {
    label: '${.replace()}',
    detail: 'Replace pattern in variable value',
    snippet: [
      '${hostname.replace(${1:pattern},${2:replacement})}',
      '${path.replace(/api/,api/v2)}',
    ],
  },
  {
    label: '$1 $2',
    detail: 'Regex capture groups',
    snippet: ['$1', '$2', '$3'],
  },
];

const clean = (token: string) => {
  return token.replace(/\${\d:([\w-]+)}/g, (_, $1) => $1);
};

let dynamicOperators: Operator[] | null = null;
let dynamicTemplateVars: Operator[] | null = null;

async function loadDynamicData(): Promise<void> {
  try {
    const syntaxInfo = await fetchSyntaxInfo();

    const filterOperators = buildFilterOperators(syntaxInfo.filter_specs);

    dynamicOperators = [
      ...syntaxInfo.protocols.map((p) => protocolToOperator(p, syntaxInfo)),
      ...filterOperators,
    ];

    const aliasOperators: Operator[] = [];
    for (const [alias, target] of Object.entries(syntaxInfo.protocol_aliases)) {
      const targetProtocol = syntaxInfo.protocols.find((p) => p.name === target);
      if (targetProtocol) {
        aliasOperators.push({
          label: alias,
          detail: `${alias}://<value> - Alias for ${target}`,
          snippet: getSnippetsForProtocol({ ...targetProtocol, name: alias }, syntaxInfo),
        });
      }
    }
    dynamicOperators.push(...aliasOperators);

    dynamicTemplateVars = [
      ...syntaxInfo.template_variables.map(templateVariableToOperator),
      ...staticTemplateExtras,
    ];
  } catch (error) {
    console.error('Failed to load syntax info:', error);
  }
}

loadDynamicData();

function getOperators(): Operator[] {
  return dynamicOperators || [];
}

function getTemplateVariables(): Operator[] {
  return dynamicTemplateVars || staticTemplateExtras;
}

const create = (range: IRange): languages.CompletionItem[] => {
  const list: languages.CompletionItem[] = [];
  const operators = getOperators();

  const base = {
    kind: languages.CompletionItemKind.Snippet,
    insertTextRules: languages.CompletionItemInsertTextRule.InsertAsSnippet,
    range,
  };

  operators.forEach(({ snippet, label, ...other }: Operator) => {
    const token = !label ? '' : clean(label as string);

    snippet.map((v: string) => {
      list.push({
        ...base,
        ...other,
        label: snippet.length > 1 ? clean(v) : token,
        insertText: v,
      } as languages.CompletionItem);
    });
  });

  return list;
};

const createTemplateVars = (range: IRange): languages.CompletionItem[] => {
  const list: languages.CompletionItem[] = [];
  const templateVariables = getTemplateVariables();

  const base = {
    kind: languages.CompletionItemKind.Variable,
    insertTextRules: languages.CompletionItemInsertTextRule.InsertAsSnippet,
    range,
  };

  templateVariables.forEach(({ snippet, label, ...other }: Operator) => {
    const token = !label ? '' : clean(label as string);

    snippet.map((v: string) => {
      list.push({
        ...base,
        ...other,
        label: snippet.length > 1 ? clean(v) : token,
        insertText: v,
      } as languages.CompletionItem);
    });
  });

  return list;
};

interface EditorContext {
  hasPattern: boolean;
  currentProtocol: string | null;
  afterProtocolSeparator: boolean;
  inCodeBlock: boolean;
  inLineBlock: boolean;
}

function analyzeContext(textBeforeCursor: string): EditorContext {
  const inCodeBlock = /```\w*\s*$/.test(textBeforeCursor) || /```\w+[\s\S]*(?!```)$/.test(textBeforeCursor);
  const inLineBlock = /(?:^|\n)line`[\s\S]*(?!\n`\s*\n)/.test(textBeforeCursor);

  const protocolMatch = textBeforeCursor.match(/(\w+):\/\/(\S*)$/);
  if (protocolMatch) {
    return {
      hasPattern: true,
      currentProtocol: protocolMatch[1],
      afterProtocolSeparator: true,
      inCodeBlock,
      inLineBlock,
    };
  }

  const hasPattern = inLineBlock || (/^\s*\S+/.test(textBeforeCursor) && !textBeforeCursor.trim().startsWith('#'));

  return {
    hasPattern,
    currentProtocol: null,
    afterProtocolSeparator: false,
    inCodeBlock,
    inLineBlock,
  };
}

function getProtocolValueSuggestions(protocol: string, range: IRange): languages.CompletionItem[] {
  const suggestions: languages.CompletionItem[] = [];
  const base = {
    kind: languages.CompletionItemKind.Value,
    range,
  };

  const protocolLower = protocol.toLowerCase();

  if (protocolLower === 'devtools') {
    return suggestions;
  }

  switch (protocolLower) {
    case 'statuscode':
    case 'replacestatus':
      HTTP_STATUS_CODES.forEach(({ code, label }) => {
        suggestions.push({
          ...base,
          label: String(code),
          detail: label,
          insertText: String(code),
          sortText: String(code).padStart(3, '0'),
        });
      });
      break;

    case 'method':
      HTTP_METHODS.forEach((method, index) => {
        suggestions.push({
          ...base,
          label: method,
          detail: `HTTP ${method} method`,
          insertText: method,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'cache':
      CACHE_VALUES.forEach(({ value, label }, index) => {
        suggestions.push({
          ...base,
          label: value,
          detail: label,
          insertText: value,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'restype':
    case 'reqtype':
      CONTENT_TYPES.forEach(({ value, label }, index) => {
        suggestions.push({
          ...base,
          label: value,
          detail: label,
          insertText: value,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'reqdelay':
    case 'resdelay':
      [100, 200, 500, 1000, 2000, 3000, 5000].forEach((ms, index) => {
        suggestions.push({
          ...base,
          label: String(ms),
          detail: ms >= 1000 ? `${ms / 1000} second${ms > 1000 ? 's' : ''}` : `${ms} milliseconds`,
          insertText: String(ms),
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'reqspeed':
    case 'resspeed':
      [1024, 10240, 51200, 102400, 512000, 1048576].forEach((bytes, index) => {
        const kb = bytes / 1024;
        suggestions.push({
          ...base,
          label: String(bytes),
          detail: kb >= 1024 ? `${kb / 1024} MB/s` : `${kb} KB/s`,
          insertText: String(bytes),
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'proxy':
      [
        { value: '127.0.0.1:8888', detail: 'HTTP proxy (localhost)' },
        { value: 'user:password@127.0.0.1:8888', detail: 'HTTP proxy with auth' },
        { value: 'socks5://127.0.0.1:1080', detail: 'SOCKS5 proxy' },
        { value: 'socks5://user:password@127.0.0.1:1080', detail: 'SOCKS5 proxy with auth' },
        { value: 'http://user:password@proxy.example.com:3128', detail: 'HTTP proxy with auth (explicit)' },
      ].forEach(({ value, detail }, index) => {
        suggestions.push({
          ...base,
          label: value,
          detail,
          insertText: value,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'rescors':
      suggestions.push(
        { ...base, label: '*', detail: 'Allow all origins', insertText: '*', sortText: '0' },
        { ...base, label: 'http://localhost:3000', detail: 'Local dev server', insertText: 'http://localhost:3000', sortText: '1' },
      );
      break;

    case 'rescharset':
    case 'reqcharset':
      ['utf-8', 'gbk', 'gb2312', 'iso-8859-1', 'utf-16'].forEach((charset, index) => {
        suggestions.push({
          ...base,
          label: charset,
          detail: `${charset} encoding`,
          insertText: charset,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    case 'trailers':
      [
        { value: '{X-Checksum:abc123}', detail: 'Checksum trailer' },
        { value: '{Server-Timing:total;dur=123}', detail: 'Server timing trailer' },
        { value: '{X-Request-Id:req-001}', detail: 'Request ID trailer' },
      ].forEach(({ value, detail }, index) => {
        suggestions.push({
          ...base,
          label: value,
          detail,
          insertText: value,
          sortText: String(index).padStart(2, '0'),
        });
      });
      break;

    default: {
      const result = getProtocolDoc(protocol);
      if (result && result.doc.examples.length > 0) {
        result.doc.examples.forEach((example: string, index: number) => {
          const value = example.split('://')[1] || '';
          if (value) {
            suggestions.push({
              ...base,
              label: value,
              detail: `Example: ${example}`,
              insertText: value,
              sortText: String(index).padStart(2, '0'),
            });
          }
        });
      }
    }
  }

  return suggestions;
}

const provider: languages.CompletionItemProvider = {
  triggerCharacters: ['$', '{', ':', '/'],
  provideCompletionItems: (model: editor.ITextModel, position: Position) => {
    let suggestions: languages.CompletionItem[] = [];
    if (model.isDisposed()) {
      return { suggestions };
    }
    const text = model.getLineContent(position.lineNumber);
    const textBeforeCursor = text.substring(0, position.column - 1);

    if (text.trim()[0] !== '@') {
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };

      const context = analyzeContext(textBeforeCursor);

      if (context.afterProtocolSeparator && context.currentProtocol) {
        suggestions = getProtocolValueSuggestions(context.currentProtocol, range);
        if (suggestions.length > 0) {
          return { suggestions };
        }
      }

      if (
        textBeforeCursor.endsWith('$') ||
        textBeforeCursor.endsWith('${') ||
        textBeforeCursor.match(/\$\{[\w.]*$/)
      ) {
        const varRange = {
          ...range,
          startColumn: Math.max(
            1,
            position.column -
            (textBeforeCursor.match(/\$\{?[\w.]*$/)?.[0]?.length || 0)
          ),
        };
        suggestions = createTemplateVars(varRange);
      } else {
        suggestions = [...create(range), ...createTemplateVars(range)];
      }
    }
    return {
      suggestions,
    };
  },
};

export default provider;

export { loadDynamicData, getCachedSyntaxInfo };
