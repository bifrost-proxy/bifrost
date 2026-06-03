import { useMemo, useRef, useState, useCallback, useEffect } from 'react';
import { theme, Button, Tooltip, message } from 'antd';
import { CopyOutlined, FormatPainterOutlined, EditOutlined } from '@ant-design/icons';
import hljs from 'highlight.js/lib/core';
import json from 'highlight.js/lib/languages/json';
import xml from 'highlight.js/lib/languages/xml';
import javascript from 'highlight.js/lib/languages/javascript';
import css from 'highlight.js/lib/languages/css';
import plaintext from 'highlight.js/lib/languages/plaintext';
import Editor from '@monaco-editor/react';
import '../../../../styles/hljs-github-theme.css';

import type { RecordContentType, SessionTargetSearchState } from '../../../../types';
import { useTextSelection } from '../../hooks/useTextSelection';
import { useMarkSearch } from '../../hooks/useMarkSearch';
import {
  getHighlightLanguage,
  DEFAULT_SHOW_MAX_SIZE,
  shouldDisableJsonStructuredView,
} from '../../helper/contentType';
import { copyToClipboard } from '../../../../utils/clipboard';
import { useThemeStore } from '../../../../stores/useThemeStore';

hljs.registerLanguage('json', json);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('html', xml);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('css', css);
hljs.registerLanguage('plaintext', plaintext);

/**
 * Pretty-print a JSON string by re-indenting the original text.
 *
 * Crucially, this never feeds numbers through `JSON.parse`/`JSON.stringify`:
 * a `JSON.parse` round-trip silently rounds any integer beyond 2^53-1 (int64
 * IDs, trace IDs, snowflake IDs) to the nearest double. By walking the source
 * text and only adjusting whitespace outside of strings, every literal —
 * numbers included — is preserved byte-for-byte.
 *
 * The input is assumed to be valid JSON (callers gate on `JSON.parse`).
 */
const reindentJson = (text: string): string => {
  const INDENT = '  ';
  let out = '';
  let depth = 0;
  let inString = false;
  let escaped = false;

  const isWhitespace = (c: string) => c === ' ' || c === '\t' || c === '\n' || c === '\r';

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];

    if (inString) {
      out += ch;
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }

    if (ch === '"') {
      inString = true;
      out += ch;
    } else if (ch === '{' || ch === '[') {
      let j = i + 1;
      while (j < text.length && isWhitespace(text[j])) j++;
      // Keep empty containers on one line: {} / [].
      if (text[j] === '}' || text[j] === ']') {
        out += ch + text[j];
        i = j;
      } else {
        depth++;
        out += ch + '\n' + INDENT.repeat(depth);
      }
    } else if (ch === '}' || ch === ']') {
      depth--;
      out += '\n' + INDENT.repeat(depth) + ch;
    } else if (ch === ',') {
      out += ',\n' + INDENT.repeat(depth);
    } else if (ch === ':') {
      out += ': ';
    } else if (!isWhitespace(ch)) {
      out += ch;
    }
  }
  return out;
};

const formatJsonContent = (text: string): { formatted: string; isJson: boolean } => {
  try {
    // Validity check only — the parsed value is discarded so that large
    // integers never lose precision through the number conversion.
    JSON.parse(text);
    return { formatted: reindentJson(text), isJson: true };
  } catch {
    return { formatted: text, isJson: false };
  }
};

interface HighLightBodyProps {
  data?: string | null;
  contentType: RecordContentType;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  editable?: boolean;
  onBodyChange?: (value: string) => void;
}

export const HighLightBody = ({
  data,
  contentType,
  searchValue,
  onSearch,
  editable = false,
  onBodyChange,
}: HighLightBodyProps) => {
  const { token } = theme.useToken();
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);

  const [showAll, setShowAll] = useState(false);
  const [isFormatted, setIsFormatted] = useState(true);
  const [isEditing, setIsEditing] = useState(false);
  const wrapperRef = useTextSelection(!!data);
  const codeRef = useRef<HTMLElement>(null);

  const isJsonType = contentType === 'JSON';
  const disableJsonStructuredView = useMemo(() => {
    return shouldDisableJsonStructuredView(contentType, data);
  }, [contentType, data]);

  const processedData = useMemo(() => {
    if (!data) return '';
    if (isJsonType && isFormatted && !disableJsonStructuredView) {
      const { formatted, isJson } = formatJsonContent(data);
      return isJson ? formatted : data;
    }
    return data;
  }, [data, disableJsonStructuredView, isJsonType, isFormatted]);

  const truncated = useMemo(() => {
    if (!processedData) return '';
    if (!showAll && processedData.length > DEFAULT_SHOW_MAX_SIZE) {
      return processedData.substring(0, DEFAULT_SHOW_MAX_SIZE);
    }
    return processedData;
  }, [processedData, showAll]);

  const highlighted = useMemo(() => {
    if (!truncated) return '';
    try {
      const lang = disableJsonStructuredView
        ? 'plaintext'
        : getHighlightLanguage(contentType);
      if (lang === 'plaintext') {
        return truncated;
      }
      const result = hljs.highlight(truncated, { language: lang });
      return result.value;
    } catch {
      return truncated;
    }
  }, [truncated, contentType, disableJsonStructuredView]);

  const shouldShowMore = !showAll && (processedData?.length ?? 0) > DEFAULT_SHOW_MAX_SIZE;

  const handleCopy = useCallback(async () => {
    if (processedData) {
      const success = await copyToClipboard(processedData);
      if (success) {
        message.success('Copied to clipboard');
      } else {
        message.error('Failed to copy');
      }
    }
  }, [processedData]);

  const toggleFormat = useCallback(() => {
    setIsFormatted((prev) => !prev);
  }, []);

  const { startMarkSearch } = useMarkSearch(
    searchValue,
    () => codeRef.current,
    onSearch
  );

  useEffect(() => {
    const el = codeRef.current;
    if (!el) return;
    el.innerHTML = highlighted;
    const debugState = globalThis as { __bifrostLogNextHljsHtml?: boolean };
    if (debugState.__bifrostLogNextHljsHtml) {
      console.log("[resume-hljs-html]", el.innerHTML);
      debugState.__bifrostLogNextHljsHtml = false;
    }
  }, [highlighted]);

  useEffect(() => {
    if (!searchValue.value) return;
    startMarkSearch();
  }, [highlighted, searchValue.value, startMarkSearch]);

  if (data == null) {
    return null;
  }

  if (editable) {
    const monacoLang = getHighlightLanguage(contentType);
    const lineCount = (data ?? '').split('\n').length;
    const editorHeight = Math.max(200, Math.min(lineCount * 20 + 16, 600));

    return (
      <div style={{ position: 'relative' }}>
        <div
          style={{
            position: 'absolute',
            top: 4,
            right: 8,
            zIndex: 1,
            display: 'flex',
            gap: 4,
          }}
        >
          <Tooltip title="Copy">
            <Button
              type="text"
              size="small"
              icon={<CopyOutlined />}
              onClick={handleCopy}
              style={{ background: token.colorBgContainer }}
            />
          </Tooltip>
        </div>
        <Editor
          height={editorHeight}
          language={monacoLang}
          theme={resolvedTheme === 'dark' ? 'vs-dark' : 'light'}
          value={data ?? ''}
          onChange={(value) => onBodyChange?.(value ?? '')}
          options={{
            minimap: { enabled: false },
            fontSize: 12,
            lineNumbers: 'on',
            scrollBeyondLastLine: false,
            automaticLayout: true,
            tabSize: 2,
            wordWrap: 'on',
            readOnly: false,
            padding: { top: 28, bottom: 4 },
            scrollbar: {
              vertical: 'auto',
              horizontal: 'auto',
            },
          }}
        />
      </div>
    );
  }

  return (
    <div ref={wrapperRef} style={{ position: 'relative' }}>
      <div
        style={{
          position: 'absolute',
          top: 4,
          right: 8,
          zIndex: 1,
          display: 'flex',
          gap: 4,
        }}
      >
        {isJsonType && !disableJsonStructuredView && (
          <Tooltip title={isFormatted ? 'Raw' : 'Format'}>
            <Button
              type="text"
              size="small"
              icon={<FormatPainterOutlined />}
              onClick={toggleFormat}
              style={{
                background: token.colorBgContainer,
                opacity: isFormatted ? 1 : 0.6,
              }}
            />
          </Tooltip>
        )}
        <Tooltip title="Copy">
          <Button
            type="text"
            size="small"
            icon={<CopyOutlined />}
            onClick={handleCopy}
            style={{ background: token.colorBgContainer }}
          />
        </Tooltip>
      </div>
      <pre
        style={{
          margin: 0,
          padding: 8,
          paddingTop: 32,
          fontSize: 12,
          fontFamily: 'monospace',
          backgroundColor: token.colorBgLayout,
          borderRadius: 4,
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-all',
          lineHeight: 1.4,
        }}
      >
        <code ref={codeRef} className="hljs" style={{ fontFamily: 'inherit' }} />
      </pre>
      {shouldShowMore && (
        <Button
          type="link"
          onClick={() => setShowAll(true)}
          style={{
            position: 'absolute',
            bottom: 8,
            right: 8,
            background: token.colorBgContainer,
          }}
        >
          Show All ({Math.round((processedData.length - DEFAULT_SHOW_MAX_SIZE) / 1024)}KB more)
        </Button>
      )}
    </div>
  );
};
