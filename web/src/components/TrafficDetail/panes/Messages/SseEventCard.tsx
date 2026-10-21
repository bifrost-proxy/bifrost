import { useMemo, useState } from 'react';
import { Typography, Tag, Button, Space, Tooltip, theme } from 'antd';
import {
  CopyOutlined,
  ExpandOutlined,
  CheckOutlined,
} from '@ant-design/icons';
import dayjs from 'dayjs';
import hljs from 'highlight.js/lib/core';
import json from 'highlight.js/lib/languages/json';
import '../../../../styles/hljs-github-theme.css';
import type { SSEEvent } from '../../../../types';
import { copyToClipboard } from '../../../../utils/clipboard';
import { useI18n } from '../../../../i18n';

hljs.registerLanguage('json', json);

const { Text } = Typography;
const MAX_SSE_EVENT_JSON_PRETTY_LENGTH = 8 * 1024;
const MAX_SSE_EVENT_HIGHLIGHT_LENGTH = 16 * 1024;
const MAX_SSE_EVENT_EXPANDABLE_LINES = 20;

const parseJson = (text: string): { parsed: unknown; isJson: boolean } => {
  try {
    const parsed = JSON.parse(text);
    return { parsed, isJson: true };
  } catch {
    return { parsed: null, isJson: false };
  }
};

const highlightJson = (text: string): string => {
  try {
    const result = hljs.highlight(text, { language: 'json' });
    return result.value;
  } catch {
    return text;
  }
};

const escapeHtml = (text: string): string => {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
};

const highlightText = (text: string, search?: string): string => {
  if (!search) return escapeHtml(text);
  const escaped = escapeHtml(text);
  const lower = text.toLowerCase();
  const lowerSearch = search.toLowerCase();
  if (!lowerSearch) return escaped;
  let result = '';
  let idx = 0;
  while (idx < lower.length) {
    const hit = lower.indexOf(lowerSearch, idx);
    if (hit === -1) {
      result += escapeHtml(text.slice(idx));
      break;
    }
    result += escapeHtml(text.slice(idx, hit));
    result += `<mark style="background-color:#ffe58f;padding:0;">${escapeHtml(
      text.slice(hit, hit + lowerSearch.length),
    )}</mark>`;
    idx = hit + lowerSearch.length;
  }
  return result;
};

interface SseEventCardProps {
  event: SSEEvent;
  index: number;
  searchValue?: string;
  onOpenDetail: () => void;
}

export const SseEventCard = ({
  event,
  index,
  searchValue,
  onOpenDetail,
}: SseEventCardProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [copied, setCopied] = useState(false);

  const eventData = useMemo(() => {
    return {
      id: event.id,
      event: event.event,
      data: event.data || '',
    };
  }, [event]);

  const canUseStructuredJson = eventData.data.length <= MAX_SSE_EVENT_JSON_PRETTY_LENGTH;
  const canHighlightJson = eventData.data.length <= MAX_SSE_EVENT_HIGHLIGHT_LENGTH;

  const { parsed, isJson } = useMemo(() => {
    if (!canUseStructuredJson) {
      return { parsed: null, isJson: false };
    }
    return parseJson(eventData.data);
  }, [canUseStructuredJson, eventData.data]);

  const formattedContent = useMemo(() => {
    if (isJson && parsed !== null) {
      return JSON.stringify(parsed, null, 2);
    }
    return eventData.data;
  }, [isJson, parsed, eventData.data]);

  const highlightedContent = useMemo(() => {
    if (!isJson || !canHighlightJson) return null;
    return highlightJson(formattedContent);
  }, [canHighlightJson, formattedContent, isJson]);

  const handleCopy = async () => {
    const success = await copyToClipboard(formattedContent);
    if (success) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  };

  const lineCount = useMemo(() => {
    let count = 1;
    for (let i = 0; i < formattedContent.length; i += 1) {
      if (formattedContent.charCodeAt(i) === 10) {
        count += 1;
      }
    }
    return count;
  }, [formattedContent]);

  const shouldTruncate = lineCount > MAX_SSE_EVENT_EXPANDABLE_LINES;
  const displayContent = shouldTruncate
    ? formattedContent.split('\n').slice(0, MAX_SSE_EVENT_EXPANDABLE_LINES).join('\n') + '\n...'
    : formattedContent;

  const eventName = eventData.event || 'message';

  return (
    <div
      data-testid="sse-event-card"
      data-event-id={eventData.id ? String(eventData.id) : undefined}
      style={{
        borderRadius: 6,
        border: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: token.colorBgContainer,
        overflow: 'hidden',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        padding: '4px 8px',
          backgroundColor: token.colorFillQuaternary,
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
        }}
      >
        <Space size={6} align="center">
          <Text type="secondary" style={{ fontSize: 10, fontFamily: 'monospace' }}>
            #{index + 1}
          </Text>
          <Text type="secondary" style={{ fontSize: 10, fontFamily: 'monospace' }}>
            {dayjs(event.timestamp).format('HH:mm:ss.SSS')}
          </Text>
          <Tag
            color="green"
            style={{
              margin: 0,
              fontSize: 10,
              lineHeight: '16px',
              padding: '0 4px',
            }}
          >
            {eventName}
          </Tag>
          {eventData.id && (
            <Text type="secondary" style={{ fontSize: 10 }}>
              id: {eventData.id}
            </Text>
          )}
          {isJson && (
            <Tag
              color="blue"
              style={{
                margin: 0,
                fontSize: 9,
                lineHeight: '14px',
                padding: '0 3px',
              }}
            >
              JSON
            </Tag>
          )}
        </Space>
        <Space size={4}>
          <Tooltip title={copied ? t('trafficDetail.messages.copiedBang') : t('common.copy')}>
            <Button
              type="text"
              size="small"
              icon={copied ? <CheckOutlined style={{ color: token.colorSuccess }} /> : <CopyOutlined />}
              onClick={handleCopy}
              disabled={!eventData.data}
              style={{ width: 24, height: 24 }}
            />
          </Tooltip>
        </Space>
      </div>
      <div style={{ padding: 0 }}>
        {eventData.data ? (
          <div>
            {isJson && highlightedContent ? (
              <pre
                style={{
                  margin: 0,
                  fontSize: 12,
                  fontFamily: 'monospace',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                  lineHeight: 1.5,
                  backgroundColor: token.colorFillQuaternary,
                  padding: 8,
                  borderRadius: 4,
                }}
              >
                <code
                  className="hljs"
                  dangerouslySetInnerHTML={{
                    __html: searchValue
                      ? highlightText(displayContent, searchValue)
                      : shouldTruncate
                        ? highlightJson(displayContent)
                        : highlightedContent,
                  }}
                />
              </pre>
            ) : (
              <pre
                style={{
                  margin: 0,
                  fontSize: 12,
                  fontFamily: 'monospace',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                  lineHeight: 1.5,
                  color: token.colorText,
                }}
              >
                <code
                  dangerouslySetInnerHTML={{
                    __html: highlightText(displayContent, searchValue),
                  }}
                />
              </pre>
            )}
            {shouldTruncate && (
              <div
                style={{
                  display: 'flex',
                  justifyContent: 'flex-start',
                  padding: '0 8px 8px',
                }}
              >
                <Button
                  type="text"
                  size="small"
                  onClick={onOpenDetail}
                  icon={<ExpandOutlined />}
                  data-testid="sse-event-toggle"
                  style={{
                    height: 20,
                    width: 24,
                    padding: 0,
                  }}
                />
              </div>
            )}
          </div>
        ) : (
          <Text type="secondary" style={{ fontSize: 12, fontStyle: 'italic' }}>
            {t('trafficDetail.messages.emptyData')}
          </Text>
        )}
      </div>
    </div>
  );
};
